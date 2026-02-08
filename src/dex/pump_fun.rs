use std::{str::FromStr, sync::Arc};
use anyhow::{anyhow, Result};
use borsh::from_slice;
use tokio::time::Instant;
use borsh_derive::{BorshDeserialize, BorshSerialize};
use colored::Colorize;
use serde::{Deserialize, Serialize};
use solana_program_pack::Pack;
use solana_sdk::{
    instruction::{AccountMeta, Instruction},
    pubkey::Pubkey,
    signature::Keypair,
    signer::Signer,
    system_program,
};
use spl_associated_token_account::{
    get_associated_token_address, get_associated_token_address_with_program_id, instruction::create_associated_token_account_idempotent,
};
use spl_token::{ui_amount_to_amount};
use crate::{
    common::{config::SwapConfig, logger::Logger},
    engine::{monitor::BondingCurveInfo, swap::{SwapDirection, SwapInType}},
};

pub const TEN_THOUSAND: u64 = 10000;
pub const TOKEN_2022_PROGRAM: &str = "TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb";  
pub const TOKEN_PROGRAM: &str = "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA";
pub const RENT_PROGRAM: &str = "SysvarRent111111111111111111111111111111111";
pub const ASSOCIATED_TOKEN_PROGRAM: &str = "ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL";
pub const PUMP_GLOBAL: &str = "4wTV1YmiEkRvAtNtsSGPtUrqRYQMe5SKy2uB4Jjaxnjf";
pub const PUMP_FEE_RECIPIENT: &str = "CebN5WGQ4jvEPvsVU4EoHEpgzq1VV7AbicfhtW4xC9iM";
pub const PUMP_FUN_PROGRAM: &str = "6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P";
// pub const PUMP_FUN_MINT_AUTHORITY: &str = "TSLvdd1pWpHVjahSpsvCXUbgwsL3JAcvokwaKt1eokM";
pub const PUMP_FEE_CONFIG: &str = "8Wf5TiAheLUqBrKXeYg2JtAFFMWtKdG2BSFgqUcPVwTt";
pub const PUMP_FEE_PROGRAM: &str = "pfeeUxB6jkeY1Hxd7CsFCAjcbHA9rWtchMGdZ6VojVZ";
pub const PUMP_EVENT_AUTHORITY: &str = "Ce6TQqeHC9p8KetsN6JsjHK7UTZk7nasjjnr7XxXp9F1";
pub const PUMP_BUY_METHOD: u64 = 16927863322537952870;
pub const PUMP_SELL_METHOD: u64 = 12502976635542562355;
pub const PUMP_FUN_CREATE_IX_DISCRIMINATOR: &[u8] = &[24, 30, 200, 40, 5, 28, 7, 119];
pub const INITIAL_VIRTUAL_SOL_RESERVES: u64 = 30_000_000_000;
pub const INITIAL_VIRTUAL_TOKEN_RESERVES: u64 = 1_073_000_000_000_000;
pub const TOKEN_TOTAL_SUPPLY: u64 = 1_000_000_000_000_000;

// Volume accumulator seeds
pub const GLOBAL_VOLUME_ACCUMULATOR_SEED: &[u8] = b"global_volume_accumulator";
pub const USER_VOLUME_ACCUMULATOR_SEED: &[u8] = b"user_volume_accumulator";

// Minimum SOL output for selling to ensure transactions always build
pub const MIN_SOL_OUTPUT_SELLING: u64 = 0;

#[derive(Clone)]
pub struct Pump {
    pub rpc_nonblocking_client: Arc<solana_client::nonblocking::rpc_client::RpcClient>,
    pub keypair: Arc<Keypair>,
    pub rpc_client: Option<Arc<solana_client::rpc_client::RpcClient>>,
}

impl Pump {
    pub fn new(
        rpc_nonblocking_client: Arc<solana_client::nonblocking::rpc_client::RpcClient>,
        rpc_client: Arc<solana_client::rpc_client::RpcClient>,
        keypair: Arc<Keypair>,
    ) -> Self {
        Self {
            rpc_nonblocking_client,
            keypair,
            rpc_client: Some(rpc_client),
        }
    }

    async fn cache_token_account(&self, _account: Pubkey) {
        // Cache removed - no-op
    }

    // Removed get_token_price method as it requires RPC calls

    /// Calculate token amount out for buy using virtual reserves
    pub fn calculate_buy_token_amount(
        sol_amount_in: u64,
        virtual_sol_reserves: u64,
        virtual_token_reserves: u64,
    ) -> u64 {
        if sol_amount_in == 0 || virtual_sol_reserves == 0 || virtual_token_reserves == 0 {
            return 0;
        }
        
        // PumpFun bonding curve formula for buy:
        // tokens_out = (sol_in * virtual_token_reserves) / (virtual_sol_reserves + sol_in)
        let sol_amount_in_u128 = sol_amount_in as u128;
        let virtual_sol_reserves_u128 = virtual_sol_reserves as u128;
        let virtual_token_reserves_u128 = virtual_token_reserves as u128;
        
        let numerator = sol_amount_in_u128.saturating_mul(virtual_token_reserves_u128);
        let denominator = virtual_sol_reserves_u128.saturating_add(sol_amount_in_u128);
        
        if denominator == 0 {
            return 0;
        }
        
        numerator.checked_div(denominator).unwrap_or(0) as u64
    }

    /// Calculate SOL amount out for sell using virtual reserves
    pub fn calculate_sell_sol_amount(
        token_amount_in: u64,
        virtual_sol_reserves: u64,
        virtual_token_reserves: u64,
    ) -> u64 {
        if token_amount_in == 0 || virtual_sol_reserves == 0 || virtual_token_reserves == 0 {
            return 0;
        }
        
        // PumpFun bonding curve formula for sell:
        // sol_out = (token_in * virtual_sol_reserves) / (virtual_token_reserves + token_in)
        let token_amount_in_u128 = token_amount_in as u128;
        let virtual_sol_reserves_u128 = virtual_sol_reserves as u128;
        let virtual_token_reserves_u128 = virtual_token_reserves as u128;
        
        let numerator = token_amount_in_u128.saturating_mul(virtual_sol_reserves_u128);
        let denominator = virtual_token_reserves_u128.saturating_add(token_amount_in_u128);
        
        if denominator == 0 {
            return 0;
        }
        
        numerator.checked_div(denominator).unwrap_or(0) as u64
    }

    /// Calculate price using virtual reserves (consistent with transaction_parser.rs)
    pub fn calculate_price_from_virtual_reserves(
        virtual_sol_reserves: u64,
        virtual_token_reserves: u64,
    ) -> f64 {
        if virtual_token_reserves == 0 {
            return 0.0;
        }
        
        // Price = virtual_sol_reserves / virtual_token_reserves / 1_000.0
        // This matches the scaling used in transaction_parser.rs for consistency
        (virtual_sol_reserves as f64) / (virtual_token_reserves as f64) / 1_000.0
    }

    // Updated build_swap_from_parsed_data method - now only uses TradeInfoFromToken data
    pub async fn build_swap_from_parsed_data(
        &self,
        trade_info: &crate::engine::transaction_parser::TradeInfoFromToken,
        swap_config: SwapConfig,
    ) -> Result<(Arc<Keypair>, Vec<Instruction>, f64)> {
        let started_time = Instant::now();
        let _logger = Logger::new("[PUMPFUN-SWAP-FROM-PARSED] => ".blue().to_string());
        
        // Basic validation - ensure we have a PumpFun transaction
        if trade_info.dex_type != crate::engine::transaction_parser::DexType::PumpFun {
            println!("Invalid transaction type, expected PumpFun ::{:?}", trade_info.dex_type);
            // return Err(anyhow!("Invalid transaction type, expected PumpFun"));
        }
        
        // Extract the essential data
        let mint_str = &trade_info.mint;
        let owner = self.keypair.pubkey();
        // let token_program_id = Pubkey::from_str(TOKEN_2022_PROGRAM).unwrap(); // always use TOKEN_2022_PROGRAM for PumpFun
        let token_program_id = Pubkey::from_str(TOKEN_2022_PROGRAM).unwrap(); // always use TOKEN_2022_PROGRAM for PumpFun
        let native_mint = spl_token::native_mint::ID;
        let pump_program = Pubkey::from_str(PUMP_FUN_PROGRAM)?;

        // Get bonding curve account addresses (calculated, no RPC)
        let bonding_curve = get_pda(&Pubkey::from_str(mint_str)?, &pump_program)?;
        // Get associated token account for bonding curve - same calculation for both Token and Token-2022
        // The Associated Token Program address is the same regardless of token program
        let associated_bonding_curve =get_associated_token_address_with_program_id(&bonding_curve, &Pubkey::from_str(mint_str)?, &token_program_id);

        // Get volume accumulator PDAs
        let global_volume_accumulator = get_global_volume_accumulator_pda(&pump_program)?;
        let user_volume_accumulator = get_user_volume_accumulator_pda(&owner, &pump_program)?;

        // Determine if this is a buy or sell operation
        // Fix bug: buy must create ATA for output token, sell must use input token ATA for source
        let (_token_in, in_ata, token_out, out_ata, pump_method) = match swap_config.swap_direction {
            SwapDirection::Buy => {
                let token_out_pubkey = Pubkey::from_str(mint_str)?;
                (
                    native_mint,
                    get_associated_token_address(&owner, &native_mint),
                    token_out_pubkey,
                    get_associated_token_address_with_program_id(&owner, &token_out_pubkey, &token_program_id),
                    PUMP_BUY_METHOD,
                )
            }
            SwapDirection::Sell => {
                let token_in_pubkey = Pubkey::from_str(mint_str)?;
                (
                    token_in_pubkey,
                    get_associated_token_address_with_program_id(&owner, &token_in_pubkey, &token_program_id),
                    native_mint,
                    get_associated_token_address(&owner, &native_mint),
                    PUMP_SELL_METHOD,
                )
            }
        };
        
        // Use price directly from TradeInfoFromToken
        let price_in_sol = trade_info.post_current_price;
        
        // Create instructions as needed
        let mut create_instruction = None;
        
        // Always ensure ATA exists for buy using idempotent creation, to avoid race/initialization errors
        // This reduces latency by eliminating token account existence checks
        if swap_config.swap_direction == SwapDirection::Buy {
            create_instruction = Some(create_associated_token_account_idempotent(
                &owner,
                &owner,
                &token_out,
                &token_program_id,
            ));
            // Optimistically cache
            self.cache_token_account(out_ata).await;
        } else {
            // For sell, check if we have tokens to sell
            // CRITICAL: First check TOKEN_HOLDINGS - if token is there, we know we have tokens
            // Don't block on cache check if token is in TOKEN_HOLDINGS
            use crate::engine::sniper::TOKEN_HOLDINGS;
            let _has_token_in_bought_list = TOKEN_HOLDINGS.contains_key(mint_str);
            
                // Token is in TOKEN_HOLDINGS - cache the ATA optimistically
                // This helps future checks even if RPC is slow
                self.cache_token_account(in_ata).await;
        }
        
        let coin_creator = match &trade_info.coin_creator {
            Some(creator) => Pubkey::from_str(creator).unwrap_or_else(|_| panic!("Invalid creator pubkey: {}", creator)),
            None => return Err(anyhow!("Coin creator not found in trade info")),
        };
        let (creator_vault, _) = Pubkey::find_program_address(
            &[b"creator-vault", coin_creator.as_ref()],
            &pump_program,
        );

        // Calculate token amount and threshold based on operation type and parsed data
        let (token_amount, sol_amount_threshold, input_accounts) = match swap_config.swap_direction {
            SwapDirection::Buy => {
                let amount_specified = ui_amount_to_amount(swap_config.amount_in, spl_token::native_mint::DECIMALS);
                let max_sol_cost = max_amount_with_slippage(amount_specified, swap_config.buy_slippage);
                
                // Use virtual reserves from trade_info for accurate calculation
                let tokens_out = Self::calculate_buy_token_amount(
                    amount_specified,
                    trade_info.virtual_sol_reserves,
                    trade_info.virtual_token_reserves,
                );
                
                _logger.log(format!("Buy calculation - SOL in: {}, Tokens out: {}, Virtual SOL: {}, Virtual Tokens: {}", 
                    amount_specified, tokens_out, trade_info.virtual_sol_reserves, trade_info.virtual_token_reserves));
                
                (
                    tokens_out,
                    max_sol_cost,
                    vec![
                        AccountMeta::new_readonly(Pubkey::from_str(PUMP_GLOBAL)?, false),   
                        AccountMeta::new(Pubkey::from_str(PUMP_FEE_RECIPIENT)?, false),
                        AccountMeta::new_readonly(Pubkey::from_str(mint_str)?, false),
                        AccountMeta::new(bonding_curve, false),
                        AccountMeta::new(associated_bonding_curve, false),
                        AccountMeta::new(out_ata, false),
                        AccountMeta::new(owner, true),
                        AccountMeta::new_readonly(system_program::id(), false),
                        AccountMeta::new_readonly(token_program_id, false),
                        AccountMeta::new(creator_vault, false),
                        AccountMeta::new_readonly(Pubkey::from_str(PUMP_EVENT_AUTHORITY)?, false),
                        AccountMeta::new_readonly(pump_program, false),
                        AccountMeta::new(global_volume_accumulator, false),
                        AccountMeta::new(user_volume_accumulator, false),
                        AccountMeta::new_readonly(Pubkey::from_str(PUMP_FEE_CONFIG)?, false),
                        AccountMeta::new_readonly(Pubkey::from_str(PUMP_FEE_PROGRAM)?, false),                        
                    ]
                )
            },
            SwapDirection::Sell => {
                // CRITICAL: First try to get token balance from TOKEN_HOLDINGS cache
                // This uses the balance recorded when verifying the buying transaction
                // Only fall back to RPC if TOKEN_HOLDINGS doesn't have it
                let actual_token_amount = {
                    use crate::engine::sniper::TOKEN_HOLDINGS;
                    
                    // Try to get balance from TOKEN_HOLDINGS first
                    if let Some(bought_info) = TOKEN_HOLDINGS.get(mint_str) {
                        let cached_balance = bought_info.current_amount;
                        _logger.log(format!("Using cached balance from TOKEN_HOLDINGS: {} tokens", cached_balance));
                        
                        // PumpFun tokens always use 6 decimals
                        let decimals = 6;
                        
                        let raw_amount = (cached_balance * 10f64.powi(decimals as i32)) as u64;
                        
                        // Apply percentage or quantity based on swap config
                        match swap_config.in_type {
                            SwapInType::Qty => {
                                // Convert UI amount to raw amount using decimals
                                ui_amount_to_amount(swap_config.amount_in, decimals)
                            },
                            SwapInType::Pct => {
                                let percentage = swap_config.amount_in.min(1.0);
                                ((percentage * raw_amount as f64) as u64).max(1) // Ensure at least 1 token
                            }
                        }
                    } else {
                        // Fall back to RPC if not in TOKEN_HOLDINGS
                        _logger.log(format!("Token not in TOKEN_HOLDINGS, falling back to RPC for mint {}", mint_str));
                        
                        match self.rpc_nonblocking_client.get_token_account(&in_ata).await {
                            Ok(Some(account)) => {
                                let amount_value = account.token_amount.amount.parse::<u64>()
                                    .map_err(|e| anyhow!("Failed to parse token amount: {}", e))?;
                                
                                // Apply percentage or quantity based on swap config
                                match swap_config.in_type {
                                    SwapInType::Qty => {
                                        // PumpFun tokens always use 6 decimals
                                        let decimals = 6;
                                        ui_amount_to_amount(swap_config.amount_in, decimals)
                                    },
                                    SwapInType::Pct => {
                                        let percentage = swap_config.amount_in.min(1.0);
                                        ((percentage * amount_value as f64) as u64).max(1) // Ensure at least 1 token
                                    }
                                }
                            },
                            Ok(None) => {
                                // Token account doesn't exist - but we know we have tokens from TOKEN_HOLDINGS
                                // This might be a timing issue or the account needs to be created
                                // Use cached balance from TOKEN_HOLDINGS as fallback
                                _logger.log(format!("⚠️ RPC says token account doesn't exist, but token is in TOKEN_HOLDINGS - using cached balance"));
                                
                                // Try to get from TOKEN_HOLDINGS again (should have been checked above, but double-check)
                                if let Some(bought_info) = TOKEN_HOLDINGS.get(mint_str) {
                                    let cached_balance = bought_info.current_amount;
                                    // PumpFun tokens always use 6 decimals
                                    let decimals = 6;
                                    let raw_amount = (cached_balance * 10f64.powi(decimals as i32)) as u64;
                                    
                                    match swap_config.in_type {
                                        SwapInType::Qty => ui_amount_to_amount(swap_config.amount_in, decimals),
                                        SwapInType::Pct => {
                                            let percentage = swap_config.amount_in.min(1.0);
                                            ((percentage * raw_amount as f64) as u64).max(1)
                                        }
                                    }
                                } else {
                                    return Err(anyhow!("Token account does not exist for mint {} and not in TOKEN_HOLDINGS", mint_str));
                                }
                            },
                            Err(e) => {
                                // RPC error - try to use TOKEN_HOLDINGS as fallback
                                _logger.log(format!("⚠️ RPC error getting token account: {:?}, trying TOKEN_HOLDINGS", e));
                                
                                if let Some(bought_info) = TOKEN_HOLDINGS.get(mint_str) {
                                    let cached_balance = bought_info.current_amount;
                                    // PumpFun tokens always use 6 decimals
                                    let decimals = 6;
                                    let raw_amount = (cached_balance * 10f64.powi(decimals as i32)) as u64;
                                    
                                    match swap_config.in_type {
                                        SwapInType::Qty => ui_amount_to_amount(swap_config.amount_in, decimals),
                                        SwapInType::Pct => {
                                            let percentage = swap_config.amount_in.min(1.0);
                                            ((percentage * raw_amount as f64) as u64).max(1)
                                        }
                                    }
                                } else {
                                    return Err(anyhow!("Failed to get token account balance: {} and not in TOKEN_HOLDINGS", e));
                                }
                            }
                        }
                    }
                };
                
                // Calculate expected SOL output using bonding curve (for logging only)
                let expected_sol_out = Self::calculate_sell_sol_amount(
                    actual_token_amount,
                    trade_info.virtual_sol_reserves,
                    trade_info.virtual_token_reserves,
                );
                
                _logger.log(format!("Sell calculation - ACTUAL tokens in: {}, Expected SOL out: {}, Min SOL out: 1 (slippage ignored), Virtual SOL: {}, Virtual Tokens: {}", 
                    actual_token_amount, expected_sol_out, trade_info.virtual_sol_reserves, trade_info.virtual_token_reserves));
                
                // Return accounts for sell
                // Set sol_amount_threshold to 1 to allow selling regardless of slippage
                (
                    actual_token_amount,
                    1,
                    vec![
                        AccountMeta::new_readonly(Pubkey::from_str(PUMP_GLOBAL)?, false),
                        AccountMeta::new(Pubkey::from_str(PUMP_FEE_RECIPIENT)?, false),
                        AccountMeta::new_readonly(Pubkey::from_str(mint_str)?, false),
                        AccountMeta::new(bonding_curve, false),
                        AccountMeta::new(associated_bonding_curve, false),
                        AccountMeta::new(in_ata, false),
                        AccountMeta::new(owner, true),
                        AccountMeta::new_readonly(system_program::id(), false),
                        AccountMeta::new(creator_vault, false),
                        AccountMeta::new_readonly(token_program_id, false),
                        AccountMeta::new_readonly(Pubkey::from_str(PUMP_EVENT_AUTHORITY)?, false),
                        AccountMeta::new_readonly(pump_program, false),
                        AccountMeta::new_readonly(Pubkey::from_str(PUMP_FEE_CONFIG)?, false),
                        AccountMeta::new_readonly(Pubkey::from_str(PUMP_FEE_PROGRAM)?, false),
                    ]
                )
            }
        };

        // Build swap instruction
        let swap_instruction = Instruction::new_with_bincode(
            pump_program,
            &(pump_method, token_amount, sol_amount_threshold),
            input_accounts,
        );
        
        // Combine all instructions
        let mut instructions = vec![];
        if let Some(create_instruction) = create_instruction {
            instructions.push(create_instruction);
        }
        if token_amount > 0 {
            instructions.push(swap_instruction);
        }
        
        // Validate we have instructions
        if instructions.is_empty() {
            return Err(anyhow!("Instructions is empty, no txn required."));
        }
        
        // Return the price from TradeInfoFromToken (already in SOL units)
        let token_price = price_in_sol;
        
        // Return the keypair, instructions, and the token price (in SOL units)
        Ok((self.keypair.clone(), instructions, token_price))
    }
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PumpInfo {
    pub mint: String,
    pub bonding_curve: String,
    pub associated_bonding_curve: String,
    pub complete: bool,
    pub virtual_sol_reserves: u64,
    pub virtual_token_reserves: u64,
    pub total_supply: u64,
}

#[derive(Debug, BorshSerialize, BorshDeserialize)]
pub struct BondingCurveAccount {
    pub discriminator: u64,
    pub virtual_token_reserves: u64,
    pub virtual_sol_reserves: u64,
    pub real_token_reserves: u64,
    pub real_sol_reserves: u64,
    pub token_total_supply: u64,
    pub complete: bool,
}

#[derive(Debug, BorshSerialize, BorshDeserialize)]
pub struct BondingCurveReserves {
    pub virtual_token_reserves: u64,
    pub virtual_sol_reserves: u64,
}

#[derive(Debug, BorshSerialize, BorshDeserialize)]
pub struct GlobalVolumeAccumulator {
    pub start_time: i64,
    pub end_time: i64,
    pub seconds_in_a_day: i64,
    pub mint: Pubkey,
    pub total_token_supply: [u64; 30],
    pub sol_volumes: [u64; 30],
}

#[derive(Debug, BorshSerialize, BorshDeserialize)]
pub struct UserVolumeAccumulator {
    pub user: Pubkey,
    pub needs_claim: bool,
    pub total_unclaimed_tokens: u64,
    pub total_claimed_tokens: u64,
    pub current_sol_volume: u64,
    pub last_update_timestamp: i64,
    pub has_total_claimed_tokens: bool,
}

pub fn get_bonding_curve_account_by_calc(
    bonding_curve_info: BondingCurveInfo,
    mint: Pubkey,
) -> (Pubkey, Pubkey, BondingCurveReserves) {
    let bonding_curve = bonding_curve_info.bonding_curve;
    let associated_bonding_curve = get_associated_token_address(&bonding_curve, &mint);
    
    let bonding_curve_reserves = BondingCurveReserves 
        { 
            virtual_token_reserves: bonding_curve_info.new_virtual_token_reserve, 
            virtual_sol_reserves: bonding_curve_info.new_virtual_sol_reserve,
        };

    (
        bonding_curve,
        associated_bonding_curve,
        bonding_curve_reserves,
    )
}

pub async fn get_bonding_curve_account(
    rpc_client: Arc<solana_client::rpc_client::RpcClient>,
    mint: Pubkey,
    pump_program: Pubkey,
) -> Result<(Pubkey, Pubkey, BondingCurveReserves)> {
    let bonding_curve = get_pda(&mint, &pump_program)?;
    let associated_bonding_curve = get_associated_token_address(&bonding_curve, &mint);
    
    // Get account data and token balance sequentially since RpcClient is synchronous
    let bonding_curve_data_result = rpc_client.get_account_data(&bonding_curve);
    let token_balance_result = rpc_client.get_token_account_balance(&associated_bonding_curve);
    
    let bonding_curve_reserves = match bonding_curve_data_result {
        Ok(ref bonding_curve_data) => {
            match from_slice::<BondingCurveAccount>(bonding_curve_data) {
                Ok(bonding_curve_account) => BondingCurveReserves {
                    virtual_token_reserves: bonding_curve_account.virtual_token_reserves,
                    virtual_sol_reserves: bonding_curve_account.virtual_sol_reserves 
                },
                Err(_) => {
                    // Fallback to direct balance checks
                    let bonding_curve_sol_balance = rpc_client.get_balance(&bonding_curve).unwrap_or(0);
                    let token_balance = match &token_balance_result {
                        Ok(balance) => {
                            match balance.ui_amount {
                                Some(amount) => (amount * (10f64.powf(balance.decimals as f64))) as u64,
                                None => 0,
                            }
                        },
                        Err(_) => 0
                    };
                    
                    BondingCurveReserves {
                        virtual_token_reserves: token_balance,
                        virtual_sol_reserves: bonding_curve_sol_balance,
                    }
                }
            }
        },
        Err(_) => {
            // Fallback to direct balance checks
            let bonding_curve_sol_balance = rpc_client.get_balance(&bonding_curve).unwrap_or(0);
            let token_balance = match &token_balance_result {
                Ok(balance) => {
                    match balance.ui_amount {
                        Some(amount) => (amount * (10f64.powf(balance.decimals as f64))) as u64,
                        None => 0,
                    }
                },
                Err(_) => 0
            };
            
            BondingCurveReserves {
                virtual_token_reserves: token_balance,
                virtual_sol_reserves: bonding_curve_sol_balance,
            }
        }
    };

    Ok((
        bonding_curve,
        associated_bonding_curve,
        bonding_curve_reserves,
    ))
}

fn max_amount_with_slippage(input_amount: u64, slippage_bps: u64) -> u64 {
    input_amount
        .checked_mul(slippage_bps.checked_add(TEN_THOUSAND).unwrap())
        .unwrap()
        .checked_div(TEN_THOUSAND)
        .unwrap()
}

pub fn get_pda(mint: &Pubkey, program_id: &Pubkey ) -> Result<Pubkey> {
    let seeds = [b"bonding-curve".as_ref(), mint.as_ref()];
    let (bonding_curve, _bump) = Pubkey::find_program_address(&seeds, program_id);
    Ok(bonding_curve)
}

/// Get the global volume accumulator PDA
pub fn get_global_volume_accumulator_pda(program_id: &Pubkey) -> Result<Pubkey> {
    let seeds = [GLOBAL_VOLUME_ACCUMULATOR_SEED];
    let (pda, _bump) = Pubkey::find_program_address(&seeds, program_id);
    Ok(pda)
}

/// Get the user volume accumulator PDA for a specific user
pub fn get_user_volume_accumulator_pda(user: &Pubkey, program_id: &Pubkey) -> Result<Pubkey> {
    let seeds = [USER_VOLUME_ACCUMULATOR_SEED, user.as_ref()];
    let (pda, _bump) = Pubkey::find_program_address(&seeds, program_id);
    Ok(pda)
}