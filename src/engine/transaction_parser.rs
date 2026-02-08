use bs58;
use std::str::FromStr;
use solana_sdk::pubkey::Pubkey;
use colored::Colorize;
use crate::common::logger::Logger;
use lazy_static;
use yellowstone_grpc_proto::geyser::SubscribeUpdateTransaction;
// Import PUMP_FUN_PROGRAM instead of PUMP_PROGRAM
// Create a static logger for this module
lazy_static::lazy_static! {
    static ref LOGGER: Logger = Logger::new("[PARSER] => ".blue().to_string());
}

#[derive(Clone, Debug, PartialEq)]
pub enum DexType {
    PumpSwap,
    PumpFun,
    Unknown,
}


#[derive(Clone, Debug)]
pub struct TradeInfoFromToken {
    // Common fields
    pub dex_type: DexType,
    pub slot: u64,
    pub signature: String,
    pub pool_id: String,
    pub mint: String,
    pub timestamp: u64,
    pub is_buy: bool,
    pub post_current_price: f64,
    pub pre_current_price: f64,
    pub is_reverse_when_pump_swap: bool,
    pub coin_creator: Option<String>,
    pub sol_change: f64,
    pub target_transaction_token_change: f64,
    pub liquidity: f64,  // this is for filtering out small trades
    pub virtual_sol_reserves: u64,
    pub virtual_token_reserves: u64,
    pub buy_sell_in_same_tx: bool,
    // always  is_token_2022: bool,
}

/// Previous transaction tracking information for detecting same-trader transactions
/// Cache is updated in real-time during gRPC streaming (no expiration needed)
#[derive(Clone, Debug)]
pub struct PreviousTransactionTrackingInfo {
    pub dex_type: DexType,
    pub slot: u64,
    pub mint: String,
    pub is_buy: bool,
    pub price: f64,
    pub sol_change: f64,
}

/// Helper function to check if transaction contains Buy instruction
fn has_buy_instruction(txn: &SubscribeUpdateTransaction) -> bool {
    if let Some(tx_inner) = &txn.transaction {
        if let Some(meta) = &tx_inner.meta {
            return meta.log_messages.iter().any(|log| {
                log.contains("Instruction: Buy")
            });
        }
    }
    false
}

/// Helper function to check if transaction contains Sell instruction
fn has_sell_instruction(txn: &SubscribeUpdateTransaction) -> bool {
    if let Some(tx_inner) = &txn.transaction {
        if let Some(meta) = &tx_inner.meta {
            return meta.log_messages.iter().any(|log| {
                log.contains("Instruction: Sell")
            });
        }
    }
    false
}

/// Parses the transaction data buffer into a TradeInfoFromToken struct
pub fn parse_transaction_data(txn: &SubscribeUpdateTransaction, buffer: &[u8]) -> Option<TradeInfoFromToken> {
    // Extract slot once and reuse
    let slot = txn.slot;
    fn parse_public_key(buffer: &[u8], offset: usize) -> Option<String> {
        if offset + 32 > buffer.len() {
            return None;
        }
        Some(bs58::encode(&buffer[offset..offset+32]).into_string())
    }

    fn parse_u64(buffer: &[u8], offset: usize) -> Option<u64> {
        if offset + 8 > buffer.len() {
            return None;
        }
        let mut bytes = [0u8; 8];
        bytes.copy_from_slice(&buffer[offset..offset+8]);
        Some(u64::from_le_bytes(bytes))
    }
    
    // Helper function to extract token mint from token balances
    fn extract_token_info(
        txn: &SubscribeUpdateTransaction,
    ) -> String {
        
        let mut mint = String::new();
        
        // Try to extract from token balances if txn is available
        if let Some(tx_inner) = &txn.transaction {
            if let Some(meta) = &tx_inner.meta {
                // Check post token balances
                if !meta.post_token_balances.is_empty() {
                    mint = meta.post_token_balances[0].mint.clone();
                    
                if mint == "So11111111111111111111111111111111111111112" {
                        if meta.post_token_balances.len() > 1 {
                            mint = meta.post_token_balances[1].mint.clone();
                            if mint == "So11111111111111111111111111111111111111112" {
                                if meta.post_token_balances.len() > 2 {
                                    mint = meta.post_token_balances[2].mint.clone();
                                }
                            }
                        }
                    }
                }
            }
        }
        
        // If we couldn't extract from token balances, use default
        if mint.is_empty() {
            mint = "2ivzYvjnKqA4X3dVvPKr7bctGpbxwrXbbxm44TJCpump".to_string();
        }
        
        mint
    }
    
    match buffer.len() {     
        368 | 416=> {  // pump swap transaction - 368 bytes
            // Extract token mint and check for reverse case
            let mint = extract_token_info(&txn);
            let timestamp = parse_u64(buffer, 16)?;
            let base_amount_in_or_base_amount_out = parse_u64(buffer, 24)?;
            // let min_quote_amount_out = parse_u64(buffer, 32)?; // Unused
            // let user_base_token_reserves = parse_u64(buffer, 40)?; // Unused
            // let user_quote_token_reserves = parse_u64(buffer, 48)?; // Unused
            let pool_base_token_reserves = parse_u64(buffer, 56)?;
            let pool_quote_token_reserves = parse_u64(buffer, 64)?;
            let quote_amount_out = parse_u64(buffer, 72)?;
            // let lp_fee_basis_points = parse_u64(buffer, 80)?; // Unused
            // let lp_fee = parse_u64(buffer, 88)?; // Unused
            // let protocol_fee_basis_points = parse_u64(buffer, 96)?; // Unused
            // let protocol_fee = parse_u64(buffer, 104)?; // Unused
            // let quote_amount_out_without_lp_fee = parse_u64(buffer, 112)?; // Unused
            // let user_quote_amount_out = parse_u64(buffer, 120)?; // Unused
            let pool_id = parse_public_key(buffer, 128)?;
            let coin_creator = parse_public_key(buffer, 320)?;
            
            // Determine if it's reverse case based on coin_creator
            let is_reverse_when_pump_swap = coin_creator == "11111111111111111111111111111111";
            
            // Calculate price based on is_reverse_when_pump_swap
            let post_current_price = if pool_base_token_reserves > 0 && pool_quote_token_reserves > 0 {
                if is_reverse_when_pump_swap {
                    // In reverse case: poolBaseTokenReserves/poolQuoteTokenReserves (base_mint is WSOL)
                    pool_base_token_reserves as f64 / pool_quote_token_reserves as f64 / 1_000.0
                } else {
                    // Normal case: poolQuoteTokenReserves/poolBaseTokenReserves (quote_mint is WSOL)
                    pool_quote_token_reserves as f64 / pool_base_token_reserves as f64 / 1_000.0
                }
            } else {
                0.0
            };

            let pre_current_price = if base_amount_in_or_base_amount_out > 0 && quote_amount_out > 0 {
                if is_reverse_when_pump_swap {
                    // In reverse case: poolBaseTokenReserves/poolQuoteTokenReserves (base_mint is WSOL)
                    base_amount_in_or_base_amount_out as f64 / quote_amount_out as f64 / 1_000.0
                } else {
                    // Normal case: poolQuoteTokenReserves/poolBaseTokenReserves (quote_mint is WSOL)
                    quote_amount_out as f64 / base_amount_in_or_base_amount_out as f64 / 1_000.0
                }
            } else {
                0.0 // fallback
            };
            
            let is_buy = if is_reverse_when_pump_swap {
                // In reverse case, buy and sell are inverted (base_mint is WSOL)
                has_sell_instruction(txn)
            } else {
                // Normal case (quote_mint is WSOL)
                has_buy_instruction(txn)
            };
            let (sol_change, token_change) = if is_reverse_when_pump_swap {
              // Reverse case: base_mint is WSOL, quote_mint is token
              if is_buy {
                // Buy: spend SOL (base), get tokens (quote) 
                // sol_change is positive for buys (matching PumpFun convention)
                (base_amount_in_or_base_amount_out as f64 / 1_000_000_000.0, quote_amount_out as f64 / 1_000_000_000.0)
              } else {
                // Sell: get SOL (base), spend tokens (quote)
                // sol_change is negative for sells (matching PumpFun convention)
                (-(base_amount_in_or_base_amount_out as f64) / 1_000_000_000.0, -(quote_amount_out as f64) / 1_000_000_000.0)
              }
            } else {
                // Normal case: quote_mint is WSOL, base_mint is token
                if is_buy {
                    // Buy: spend SOL (quote), get tokens (base)
                    // sol_change is positive for buys (matching PumpFun convention)
                    (quote_amount_out as f64 / 1_000_000_000.0, base_amount_in_or_base_amount_out as f64 / 1_000_000_000.0)
                } else {
                    // Sell: get SOL (quote), spend tokens (base)
                    // sol_change is negative for sells (matching PumpFun convention)
                    (-(quote_amount_out as f64) / 1_000_000_000.0, -(base_amount_in_or_base_amount_out as f64) / 1_000_000_000.0)
                }
            };  

            let liquidity = if !is_reverse_when_pump_swap {
                pool_quote_token_reserves as f64 / 1_000_000_000.0
            } else {
                pool_base_token_reserves as f64 / 1_000_000_000.0
            };
            
            Some(TradeInfoFromToken {
                dex_type: DexType::PumpSwap,
                slot: 0, // Will be set from transaction data
                signature: String::new(), // Will be set from transaction data
                pool_id: pool_id.clone(),
                mint: mint.clone(),
                timestamp,
                is_buy,
                post_current_price,
                pre_current_price,
                is_reverse_when_pump_swap,
                coin_creator: Some(coin_creator),
                sol_change,
                target_transaction_token_change: token_change,
                liquidity,
                // Map pool reserves to virtual reserves as requested
                virtual_sol_reserves: pool_quote_token_reserves,  
                virtual_token_reserves: pool_base_token_reserves,  
                buy_sell_in_same_tx: false,
            })
        },

        274 | 275 => {
            // Parse PumpFunData fields
            let mint = parse_public_key(buffer, 16)?;
            let sol_amount = parse_u64(buffer, 48)?;
            let token_amount = parse_u64(buffer, 56)?;
            let is_buy = buffer.get(64)? == &1;
            let timestamp = parse_u64(buffer, 97)?;
            let virtual_sol_reserves = parse_u64(buffer, 105)?;
            let virtual_token_reserves = parse_u64(buffer, 113)?;
            let real_sol_reserves = parse_u64(buffer, 121)?;
            // let real_token_reserves = parse_u64(buffer, 129)?; // Unused
            let creator = parse_public_key(buffer, 185)?;
            // Detect mixed buy/sell instructions present in the same transaction (market-making risk)
            let mixed_buy_sell = has_buy_instruction(txn) && has_sell_instruction(txn);
            // For DEX monitoring, use virtual reserves-derived price (post-tx) from Anchor CPI logs
            let post_current_price = crate::dex::pump_fun::Pump::calculate_price_from_virtual_reserves(
                virtual_sol_reserves,
                virtual_token_reserves,
            );
            let pre_current_price = if token_amount == 0 {
                0.0
            } else {
                sol_amount as f64 / token_amount as f64 / 1_000.0
            };
        

            // Pump fun don't have pool, just have bonding curve
            let liquidity = real_sol_reserves as f64 / 1_000_000_000.0;
            let sol_change = if is_buy {
                // Buy: sol_change is positive (+)
                sol_amount as f64 / 1_000_000_000.0
            } else {
                // Sell: sol_change is negative (-)
                -(sol_amount as f64) / 1_000_000_000.0
            };

            // Suppress parser-level logs to avoid noise for non-owned tokens
            
            Some(TradeInfoFromToken {
                dex_type: DexType::PumpFun,
                slot,
                signature: String::new(), // Will be set from transaction data
                pool_id: String::new(),
                mint,
                timestamp,
                is_buy,
                post_current_price,
                pre_current_price,
                is_reverse_when_pump_swap: false, // PumpFun is never reverse
                coin_creator: Some(creator),
                sol_change,
                target_transaction_token_change: token_amount as f64 / 1_000_000.0,
                liquidity,
                virtual_sol_reserves: virtual_sol_reserves,
                virtual_token_reserves: virtual_token_reserves,
                buy_sell_in_same_tx: mixed_buy_sell,
            })
        },
        
        _ => None,
    }
}
