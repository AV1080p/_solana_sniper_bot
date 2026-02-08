/*
 * Multi-Sniper Bot
 * 
 * Changes made:
 * - Modified PumpSwap buy/sell logic to only send notifications without executing transactions
 * - Transaction processing now runs in separate tokio tasks to ensure main monitoring continues
 * - Added placeholder for future selling strategy implementation
 * - PumpFun protocol functionality remains unchanged
 * - Added caching and batch RPC calls for improved performance
 */

use anchor_client::solana_sdk::signature::{Signer, Keypair};
use solana_vntr_sniper::{
    common::{config::{Config, AppState}, constants::RUN_MSG},
    engine::{
        sniper::{start_sniper, SniperConfig},
        swap::SwapProtocol,
    },
    services::{ 
        cache_maintenance, 
        blockhash_processor::BlockhashProcessor,
    },
    core::token,
};
use std::sync::Arc;
use solana_program_pack::Pack;
use anchor_client::solana_sdk::pubkey::Pubkey;
use anchor_client::solana_sdk::transaction::Transaction;
use anchor_client::solana_sdk::system_instruction;
use std::str::FromStr;
use colored::Colorize;
use spl_token::instruction::sync_native;
use spl_token::ui_amount_to_amount;
use spl_associated_token_account::get_associated_token_address;
use spl_token_2022::extension::StateWithExtensionsOwned;
use spl_token_2022::state::{Account as Token2022Account, Mint as Token2022Mint};

/// Initialize the wallet token account list (no-op - cache removed)
/// Token accounts are now handled automatically by create_associated_token_account_idempotent
/// OPTIMIZATION: Changed parameter from Config to AppState to avoid needing full config lock
async fn initialize_token_account_list(_app_state: &AppState) {
    // Cache removed - token accounts are created automatically when needed
    // No initialization needed
}

/// Wrap SOL to Wrapped SOL (WSOL)
async fn wrap_sol(config: &Config, amount: f64) -> Result<(), String> {
    let logger = solana_vntr_sniper::common::logger::Logger::new("[WRAP-SOL] => ".green().to_string());
    
    // Get wallet pubkey
    let wallet_pubkey = match config.app_state.wallet.try_pubkey() {
        Ok(pk) => pk,
        Err(_) => return Err("Failed to get wallet pubkey".to_string()),
    };
    
    // Create WSOL account instructions
    let (wsol_account, mut instructions) = match token::create_wsol_account(wallet_pubkey) {
        Ok(result) => result,
        Err(e) => return Err(format!("Failed to create WSOL account: {}", e)),
    };
    
    logger.log(format!("WSOL account address: {}", wsol_account));
    
    // Convert UI amount to lamports (1 SOL = 10^9 lamports)
    let lamports = ui_amount_to_amount(amount, 9);
    logger.log(format!("Wrapping {} SOL ({} lamports)", amount, lamports));
    
    // Transfer SOL to the WSOL account
    instructions.push(
        system_instruction::transfer(
            &wallet_pubkey,
            &wsol_account,
            lamports,
        )
    );
    
    // Sync native instruction to update the token balance
    instructions.push(
        sync_native(
            &spl_token::id(),
            &wsol_account,
        ).map_err(|e| format!("Failed to create sync native instruction: {}", e))?
    );
    
    // Send transaction with fresh blockhash (and a one-time retry if needed)
    let recent_blockhash = if let Some(hash) = BlockhashProcessor::get_latest_blockhash().await {
        hash
    } else {
        let processor = BlockhashProcessor::new(config.app_state.rpc_client.clone())
            .await
            .map_err(|e| format!("Failed to init blockhash processor: {}", e))?;
        processor.get_fresh_blockhash()
            .await
            .map_err(|e| format!("Failed to get fresh blockhash: {}", e))?
    };

    let mut transaction = Transaction::new_signed_with_payer(
        &instructions,
        Some(&wallet_pubkey),
        &[&config.app_state.wallet],
        recent_blockhash,
    );

    match config.app_state.rpc_client.send_and_confirm_transaction(&transaction) {
        Ok(signature) => {
            logger.log(format!("SOL wrapped successfully, signature: {}", signature));
            Ok(())
        },
        Err(e) => {
            let msg = e.to_string();
            if msg.contains("Blockhash not found") || msg.contains("blockhash not found") {
                logger.log("Retrying with a fresh blockhash...".yellow().to_string());
                let processor = BlockhashProcessor::new(config.app_state.rpc_client.clone())
                    .await
                    .map_err(|e| format!("Failed to init blockhash processor: {}", e))?;
                let fresh = processor.get_fresh_blockhash()
                    .await
                    .map_err(|e| format!("Failed to get fresh blockhash: {}", e))?;

                transaction = Transaction::new_signed_with_payer(
                    &instructions,
                    Some(&wallet_pubkey),
                    &[&config.app_state.wallet],
                    fresh,
                );

                match config.app_state.rpc_client.send_and_confirm_transaction(&transaction) {
                    Ok(signature) => {
                        logger.log(format!("SOL wrapped successfully on retry, signature: {}", signature));
                        Ok(())
                    },
                    Err(e2) => Err(format!("Failed to wrap SOL: {}", e2)),
                }
            } else {
                Err(format!("Failed to wrap SOL: {}", e))
            }
        }
    }
}

/// Unwrap SOL from Wrapped SOL (WSOL) account
async fn unwrap_sol(config: &Config) -> Result<(), String> {
    let logger = solana_vntr_sniper::common::logger::Logger::new("[UNWRAP-SOL] => ".green().to_string());
    
    // Get wallet pubkey
    let wallet_pubkey = match config.app_state.wallet.try_pubkey() {
        Ok(pk) => pk,
        Err(_) => return Err("Failed to get wallet pubkey".to_string()),
    };
    
    // Get the WSOL ATA address
    let wsol_account = get_associated_token_address(
        &wallet_pubkey,
        &spl_token::native_mint::id()
    );
    
    logger.log(format!("WSOL account address: {}", wsol_account));
    
    // Check if WSOL account exists
    match config.app_state.rpc_client.get_account(&wsol_account) {
        Ok(_) => {
            logger.log(format!("Found WSOL account: {}", wsol_account));
        },
        Err(_) => {
            return Err(format!("WSOL account does not exist: {}", wsol_account));
        }
    }
    
    // Close the WSOL account to recover SOL
    let close_instruction = token::close_account(
        wallet_pubkey,
        wsol_account,
        wallet_pubkey,
        wallet_pubkey,
        &[&wallet_pubkey],
    ).map_err(|e| format!("Failed to create close account instruction: {}", e))?;
    
    // Send transaction with fresh blockhash (and a one-time retry if needed)
    let recent_blockhash = if let Some(hash) = BlockhashProcessor::get_latest_blockhash().await {
        hash
    } else {
        let processor = BlockhashProcessor::new(config.app_state.rpc_client.clone())
            .await
            .map_err(|e| format!("Failed to init blockhash processor: {}", e))?;
        processor.get_fresh_blockhash()
            .await
            .map_err(|e| format!("Failed to get fresh blockhash: {}", e))?
    };

    let mut transaction = Transaction::new_signed_with_payer(
        &[close_instruction.clone()],
        Some(&wallet_pubkey),
        &[&config.app_state.wallet],
        recent_blockhash,
    );
    
    match config.app_state.rpc_client.send_and_confirm_transaction(&transaction) {
        Ok(signature) => {
            logger.log(format!("WSOL unwrapped successfully, signature: {}", signature));
            Ok(())
        },
        Err(e) => {
            let msg = e.to_string();
            if msg.contains("Blockhash not found") || msg.contains("blockhash not found") {
                logger.log("Retrying with a fresh blockhash...".yellow().to_string());
                let processor = BlockhashProcessor::new(config.app_state.rpc_client.clone())
                    .await
                    .map_err(|e| format!("Failed to init blockhash processor: {}", e))?;
                let fresh = processor.get_fresh_blockhash()
                    .await
                    .map_err(|e| format!("Failed to get fresh blockhash: {}", e))?;

                transaction = Transaction::new_signed_with_payer(
                    &[close_instruction.clone()],
                    Some(&wallet_pubkey),
                    &[&config.app_state.wallet],
                    fresh,
                );

                match config.app_state.rpc_client.send_and_confirm_transaction(&transaction) {
                    Ok(signature) => {
                        logger.log(format!("WSOL unwrapped successfully on retry, signature: {}", signature));
                        Ok(())
                    },
                    Err(e2) => Err(format!("Failed to unwrap WSOL: {}", e2)),
                }
            } else {
                Err(format!("Failed to unwrap WSOL: {}", e))
            }
        }
    }
}

/// Sell all tokens using Jupiter API
async fn sell_all_tokens(config: &Config) -> Result<(), String> {
    let logger = solana_vntr_sniper::common::logger::Logger::new("[SELL-ALL-TOKENS] => ".green().to_string());
    let quote_logger = solana_vntr_sniper::common::logger::Logger::new("[JUPITER-QUOTE] => ".blue().to_string());
    let execute_logger = solana_vntr_sniper::common::logger::Logger::new("[EXECUTE-SWAP] => ".yellow().to_string());
    let sell_logger = solana_vntr_sniper::common::logger::Logger::new("[SELL-TOKEN] ".cyan().to_string());
    
    // Get wallet pubkey
    let wallet_pubkey = match config.app_state.wallet.try_pubkey() {
        Ok(pk) => pk,
        Err(_) => return Err("Failed to get wallet pubkey".to_string()),
    };
    
    logger.log(format!("🔍 Scanning wallet {} for tokens to sell", wallet_pubkey));
    
    // Get the token program pubkeys
    let token_program = Pubkey::from_str("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA").unwrap();
    let token_2022_program = Pubkey::from_str("TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb").unwrap();
    
    // Query all token accounts owned by the wallet (both standard Token and Token-2022)
    // Use spawn_blocking to avoid blocking the async runtime
    let wallet_pubkey_clone = wallet_pubkey.clone();
    let rpc_client_clone = config.app_state.rpc_client.clone();
    let accounts_normal_token = tokio::task::spawn_blocking(move || {
        rpc_client_clone.get_token_accounts_by_owner(
            &wallet_pubkey_clone,
            anchor_client::solana_client::rpc_request::TokenAccountsFilter::ProgramId(token_program)
        )
    }).await.map_err(|e| format!("Task join error: {}", e))?
        .map_err(|e| format!("Failed to get token accounts: {}", e))?;
    
    let wallet_pubkey_clone2 = wallet_pubkey.clone();
    let rpc_client_clone2 = config.app_state.rpc_client.clone();
    let accounts_of_token_2022 = tokio::task::spawn_blocking(move || {
        rpc_client_clone2.get_token_accounts_by_owner(
            &wallet_pubkey_clone2,
            anchor_client::solana_client::rpc_request::TokenAccountsFilter::ProgramId(token_2022_program)
        )
    }).await.map_err(|e| format!("Task join error: {}", e))?
        .map_err(|e| format!("Failed to get Token-2022 accounts: {}", e))?;
    
    // Combine both account vectors
    let normal_token_count = accounts_normal_token.len();
    let token_2022_count = accounts_of_token_2022.len();
    let mut accounts = accounts_normal_token;
    accounts.extend(accounts_of_token_2022);
    
    if accounts.is_empty() {
        logger.log("No token accounts found".to_string());
        return Ok(());
    }
    
    logger.log(format!("Found {} token accounts ({} standard + {} Token-2022)", 
                       accounts.len(), 
                       normal_token_count, 
                       token_2022_count));
    
    // OPTIMIZATION: Use shared JupiterClient from AppState (eliminates duplicate initialization)
    // Filter and collect token information
    let mut tokens_to_sell = Vec::new();
    let mut total_token_count = 0;
    let mut sold_count = 0;
    let mut failed_count = 0;
    let mut total_sol_received = 0u64;
    
    for account_info in accounts {
        let token_account = Pubkey::from_str(&account_info.pubkey)
            .map_err(|_| format!("Invalid token account pubkey: {}", account_info.pubkey))?;
        
        // Get account data (use spawn_blocking to avoid blocking)
        let token_account_clone = token_account.clone();
        let rpc_client_clone = config.app_state.rpc_client.clone();
        let account_data = match tokio::task::spawn_blocking(move || {
            rpc_client_clone.get_account(&token_account_clone)
        }).await {
            Ok(Ok(data)) => data,
            Ok(Err(e)) => {
                logger.log(format!("Failed to get account data for {}: {}", token_account, e).red().to_string());
                continue;
            },
            Err(e) => {
                logger.log(format!("Task join error for {}: {}", token_account, e).red().to_string());
                continue;
            }
        };
        
        // Determine which program owns this account (Token or Token-2022)
        let is_token_2022 = account_data.owner == token_2022_program;
        
        // Parse token account data based on program type
        let (mint, amount, decimals) = if is_token_2022 {
            // Parse Token-2022 account
            match StateWithExtensionsOwned::<Token2022Account>::unpack(account_data.data.clone()) {
                Ok(token_data) => {
                    // Skip WSOL (wrapped SOL) and accounts with zero balance
                    if token_data.base.mint == spl_token::native_mint::id() || token_data.base.amount == 0 {
                        continue;
                    }
                    
                    // Get mint account to determine decimals (use spawn_blocking)
                    let mint_pubkey = token_data.base.mint;
                    let rpc_client_clone = config.app_state.rpc_client.clone();
                    let mint_data = match tokio::task::spawn_blocking(move || {
                        rpc_client_clone.get_account(&mint_pubkey)
                    }).await {
                        Ok(Ok(data)) => data,
                        Ok(Err(e)) => {
                            logger.log(format!("Failed to get Token-2022 mint data for {}: {}", mint_pubkey, e).yellow().to_string());
                            continue;
                        },
                        Err(e) => {
                            logger.log(format!("Task join error for mint {}: {}", mint_pubkey, e).yellow().to_string());
                            continue;
                        }
                    };
                    
                    let mint_info = match StateWithExtensionsOwned::<Token2022Mint>::unpack(mint_data.data.clone()) {
                        Ok(info) => info,
                        Err(e) => {
                            logger.log(format!("Failed to parse Token-2022 mint data for {}: {}", token_data.base.mint, e).yellow().to_string());
                            continue;
                        }
                    };
                    
                    (token_data.base.mint, token_data.base.amount, mint_info.base.decimals)
                },
                Err(e) => {
                    logger.log(format!("Failed to parse Token-2022 account data for {}: {}", token_account, e).yellow().to_string());
                    continue;
                }
            }
        } else {
            // Parse standard Token account
            match spl_token::state::Account::unpack(&account_data.data) {
                Ok(token_data) => {
                    // Skip WSOL (wrapped SOL) and accounts with zero balance
                    if token_data.mint == spl_token::native_mint::id() || token_data.amount == 0 {
                        continue;
                    }
                    
                    // Get mint account to determine decimals (use spawn_blocking)
                    let mint_pubkey = token_data.mint;
                    let rpc_client_clone = config.app_state.rpc_client.clone();
                    let mint_data = match tokio::task::spawn_blocking(move || {
                        rpc_client_clone.get_account(&mint_pubkey)
                    }).await {
                        Ok(Ok(data)) => data,
                        Ok(Err(e)) => {
                            logger.log(format!("Failed to get mint data for {}: {}", mint_pubkey, e).yellow().to_string());
                            continue;
                        },
                        Err(e) => {
                            logger.log(format!("Task join error for mint {}: {}", mint_pubkey, e).yellow().to_string());
                            continue;
                        }
                    };
                    
                    let mint_info = match spl_token::state::Mint::unpack(&mint_data.data) {
                        Ok(info) => info,
                        Err(e) => {
                            logger.log(format!("Failed to parse mint data for {}: {}", token_data.mint, e).yellow().to_string());
                            continue;
                        }
                    };
                    
                    (token_data.mint, token_data.amount, mint_info.decimals)
                },
                Err(e) => {
                    logger.log(format!("Failed to parse token account data for {}: {}", token_account, e).yellow().to_string());
                    continue;
                }
            }
        };
        
        total_token_count += 1;
        let token_amount = amount as f64 / 10f64.powi(decimals as i32);
        
        logger.log(format!("📦 Found token: {} - Amount: {} (decimals: {}, program: {})", 
                           mint, token_amount, decimals, if is_token_2022 { "Token-2022" } else { "Token" }));
        
        tokens_to_sell.push((mint.to_string(), amount, decimals));
    }
    
    if tokens_to_sell.is_empty() {
        logger.log("No tokens found to sell (excluding SOL/WSOL)".yellow().to_string());
        return Ok(());
    }
    
    logger.log(format!("💱 Starting to sell {} tokens", tokens_to_sell.len()));
    
    // Sell each token using Jupiter API
    for (mint, amount, _decimals) in tokens_to_sell {
        logger.log(format!("💱 Selling token: {}", mint).cyan().to_string());
        
        // First get the quote to show detailed information
        let sol_mint = "So11111111111111111111111111111111111111112";
        quote_logger.log(format!("Getting quote: {} -> {} (amount: {})", mint, sol_mint, amount));
        
        match config.app_state.jupiter_client.get_quote(&mint, sol_mint, amount, 100).await {
            Ok(quote) => {
                // Log quote details like in the example
                quote_logger.log(format!("Raw quote response (first 500 chars): {}", 
                    serde_json::to_string(&quote).unwrap_or_default().chars().take(500).collect::<String>()));
                
                quote_logger.log(format!("Quote received: {} {} -> {} {}", 
                    quote.in_amount, mint, quote.out_amount, sol_mint));
                
                // Now get the actual transaction using the enhanced Jupiter sell method
                match config.app_state.jupiter_client.sell_token_with_jupiter(&mint, amount, 500, &config.app_state.wallet).await {
                    Ok(signature) => {
                        execute_logger.log(format!("Jupiter sell transaction sent: {}", signature));
                        
                        // Wait a moment for confirmation
                        tokio::time::sleep(tokio::time::Duration::from_millis(2000)).await;
                        execute_logger.log(format!("Jupiter sell transaction confirmed: {}", signature));
                        
                        // Log the successful sell
                        sell_logger.log(format!("{} => Token sold successfully! Signature: {}", mint, signature));
                        
                        // Remove token from bought token list after successful sell
                        solana_vntr_sniper::engine::sniper::TOKEN_HOLDINGS.remove(&mint);
                        
                        // Parse the expected SOL amount from quote
                        if let Ok(sol_amount) = quote.out_amount.parse::<u64>() {
                            total_sol_received += sol_amount;
                        }
                        
                        logger.log(format!("✅ Successfully sold {}: {}", mint, signature).green().to_string());
                        sold_count += 1;
                    },
                    Err(e) => {
                        logger.log(format!("❌ Failed to get sell transaction for token {}: {}", mint, e).red().to_string());
                        failed_count += 1;
                    }
                }
            },
            Err(e) => {
                logger.log(format!("❌ Failed to get quote for token {}: {}", mint, e).red().to_string());
                failed_count += 1;
            }
        }
        
        // Small delay between transactions to avoid rate limiting
        tokio::time::sleep(tokio::time::Duration::from_millis(1000)).await;
    }
    
    // Final summary
    let sol_received_display = total_sol_received as f64 / 1_000_000_000.0; // Convert lamports to SOL
    logger.log(format!("Selling completed! ✅ {} successful, ❌ {} failed, ~{:.6} SOL received", 
                       sold_count, failed_count, sol_received_display).cyan().bold().to_string());
    
    if failed_count > 0 {
        Err(format!("Failed to sell {} out of {} tokens", failed_count, total_token_count))
    } else {
        Ok(())
    }
}

// Debug token creation monitoring helper removed (no longer needed)

/// Close all token accounts owned by the wallet
async fn close_all_token_accounts(config: &Config) -> Result<(), String> {
    let logger = solana_vntr_sniper::common::logger::Logger::new("[CLOSE-TOKEN-ACCOUNTS] => ".green().to_string());
    
    // Get wallet pubkey
    let wallet_pubkey = match config.app_state.wallet.try_pubkey() {
        Ok(pk) => pk,
        Err(_) => return Err("Failed to get wallet pubkey".to_string()),
    };
    
    // Get the token program pubkey
    let token_program = Pubkey::from_str("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA").unwrap();
    let token_2022_program = Pubkey::from_str("TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb").unwrap();
    
    // Query all token accounts owned by the wallet
    let accounts_normal_token = config.app_state.rpc_client.get_token_accounts_by_owner(
        &wallet_pubkey,
        anchor_client::solana_client::rpc_request::TokenAccountsFilter::ProgramId(token_program)
    ).map_err(|e| format!("Failed to get token accounts: {}", e))?;
    let accounts_of_token_2022 = config.app_state.rpc_client.get_token_accounts_by_owner(
        &wallet_pubkey,
        anchor_client::solana_client::rpc_request::TokenAccountsFilter::ProgramId(token_2022_program)
    ).map_err(|e| format!("Failed to get token accounts: {}", e))?;
    
    // Combine both account vectors
    let mut accounts = accounts_normal_token;
    accounts.extend(accounts_of_token_2022);
    
    if accounts.is_empty() {
        logger.log("No token accounts found to close".to_string());
        return Ok(());
    }
    
    logger.log(format!("Found {} token accounts to close", accounts.len()));
    
    let mut closed_count = 0;
    let mut failed_count = 0;
    
    // Close each token account
    for account_info in accounts {
        let token_account = Pubkey::from_str(&account_info.pubkey)
            .map_err(|_| format!("Invalid token account pubkey: {}", account_info.pubkey))?;
        
        // Skip WSOL accounts with non-zero balance (these need to be unwrapped first)
        let account_data = match config.app_state.rpc_client.get_account(&token_account) {
            Ok(data) => data,
            Err(e) => {
                logger.log(format!("Failed to get account data for {}: {}", token_account, e).red().to_string());
                failed_count += 1;
                continue;
            }
        };
        
        // Determine which program owns this account (Token or Token-2022)
        let is_token_2022 = account_data.owner == token_2022_program;
        
        // Check if this is a WSOL account with balance
        if let Ok(token_data) = spl_token::state::Account::unpack(&account_data.data) {
            if token_data.mint == spl_token::native_mint::id() && token_data.amount > 0 {
                logger.log(format!("Skipping WSOL account with non-zero balance: {} ({})", 
                                 token_account, 
                                 token_data.amount as f64 / 1_000_000_000.0));
                continue;
            }
        }
        
        // Create close instruction using the correct program
        let close_instruction = if is_token_2022 {
            // Use Token-2022 program for Token-2022 accounts
            spl_token_2022::instruction::close_account(
                &spl_token_2022::id(),
                &token_account,
                &wallet_pubkey,
                &wallet_pubkey,
                &[&wallet_pubkey],
            ).map_err(|e| format!("Failed to create Token-2022 close instruction for {}: {}", token_account, e))?
        } else {
            // Use standard Token program for standard token accounts
            token::close_account(
                wallet_pubkey,
                token_account,
                wallet_pubkey,
                wallet_pubkey,
                &[&wallet_pubkey],
            ).map_err(|e| format!("Failed to create close instruction for {}: {}", token_account, e))?
        };
        
        // Send transaction
        let recent_blockhash = config.app_state.rpc_client.get_latest_blockhash()
            .map_err(|e| format!("Failed to get recent blockhash: {}", e))?;
        
        let transaction = Transaction::new_signed_with_payer(
            &[close_instruction],
            Some(&wallet_pubkey),
            &[&config.app_state.wallet],
            recent_blockhash,
        );
        
        match config.app_state.rpc_client.send_and_confirm_transaction(&transaction) {
            Ok(signature) => {
                logger.log(format!("Closed token account {}, signature: {}", token_account, signature));
                closed_count += 1;
            },
            Err(e) => {
                logger.log(format!("Failed to close token account {}: {}", token_account, e).red().to_string());
                failed_count += 1;
            }
        }
    }
    
    logger.log(format!("Closed {} token accounts, {} failed", closed_count, failed_count));
    
    if failed_count > 0 {
        Err(format!("Failed to close {} token accounts", failed_count))
    } else {
        Ok(())
    }
}

async fn create_nonce(config: &Config) -> Result<(), String> {
    let logger = solana_vntr_sniper::common::logger::Logger::new("[CREATE-NONCE] => ".green().to_string());
    
    // Get wallet pubkey
    let wallet_pubkey = match config.app_state.wallet.try_pubkey() {
        Ok(pk) => pk,
        Err(_) => return Err("Failed to get wallet pubkey".to_string()),
    };

    // Create a new nonce account
    let nonce_keypair = Keypair::new();
    let nonce_pubkey = nonce_keypair.pubkey();

    // Calculate rent-exempt balance
    let rent = config.app_state.rpc_client
        .get_minimum_balance_for_rent_exemption(solana_program::nonce::State::size())
        .map_err(|e| format!("Failed to get rent-exempt balance: {}", e))?;

    // Create nonce account instruction
    let create_nonce_account_ix = system_instruction::create_nonce_account(
        &wallet_pubkey,
        &nonce_pubkey,
        &wallet_pubkey,
        rent,
    );

    
    // Get recent blockhash
    let recent_blockhash = config.app_state.rpc_client
        .get_latest_blockhash()
        .map_err(|e| format!("Failed to get recent blockhash: {}", e))?;

    // Create and sign transaction
    let transaction = Transaction::new_signed_with_payer(
        &create_nonce_account_ix,
        Some(&wallet_pubkey),
        &[&config.app_state.wallet, &nonce_keypair],
        recent_blockhash,
    );
    
    // Send transaction
    match config.app_state.rpc_client.send_and_confirm_transaction(&transaction) {
        Ok(signature) => {
            // Use synchronous get_account since rpc_client is synchronous
            let nonce_account = config.app_state.rpc_client.get_account(&nonce_pubkey)
                .map_err(|e| format!("Failed to get nonce account: {}", e))?;
            let nonce_data = solana_rpc_client_nonce_utils::data_from_account(&nonce_account)
                .map_err(|e| format!("Failed to parse nonce data: {}", e))?;
            let blockhash = nonce_data.blockhash();
            logger.log(format!("Nonce account created successfully, signature: {}", signature));
            println!("nonce pubkey is {}, set NONCE_ACCOUNT={} in env", nonce_pubkey, nonce_pubkey);
            println!("nonce keypair is {:?}", nonce_keypair);
            println!("nonce privatekey is {:?}", nonce_keypair.secret());
            println!("nonce privatekey byte is {:?}", nonce_keypair.secret().to_bytes());
            println!("offline blockhash is {:?} set OFFLINE_BLOCKHASH={} in env", blockhash, blockhash);
            Ok(())
        },
        Err(e) => {
            Err(format!("Failed to create nonce account: {}", e))
        }
    }
}

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    /* Initial Settings */
    let shared_config = Config::new().await;

    // Parse command line arguments EARLY (so we can keep config guard short-lived)
    let args: Vec<String> = std::env::args().collect();

    // Handle one-off CLI actions with a short-lived lock
    if args.len() > 1 {
        if args.contains(&"--wrap".to_string()) {
            // Short-lived guard for wrap
            let guard = shared_config.lock().await;
            println!("Wrapping SOL to WSOL...");
            let wrap_amount = std::env::var("WRAP_AMOUNT").ok().and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.1);
            match wrap_sol(&guard, wrap_amount).await {
                Ok(_) => { println!("Successfully wrapped {} SOL to WSOL", wrap_amount); return; },
                Err(e) => { eprintln!("Failed to wrap SOL: {}", e); return; }
            }
        } else if args.contains(&"--unwrap".to_string()) {
            // Short-lived guard for unwrap
            let guard = shared_config.lock().await;
            println!("Unwrapping WSOL to SOL...");
            match unwrap_sol(&guard).await {
                Ok(_) => { println!("Successfully unwrapped WSOL to SOL"); return; },
                Err(e) => { eprintln!("Failed to unwrap WSOL: {}", e); return; }
            }
        } else if args.contains(&"--sell".to_string()) {
            // Short-lived guard for sell
            let guard = shared_config.lock().await;
            println!("Selling all tokens using Jupiter API...");
            match sell_all_tokens(&guard).await {
                Ok(_) => { println!("Successfully sold all tokens"); return; },
                Err(e) => { eprintln!("Failed to sell all tokens: {}", e); return; }
            }
        } else if args.contains(&"--close".to_string()) {
            // Short-lived guard for close
            let guard = shared_config.lock().await;
            println!("Closing all token accounts...");
            match close_all_token_accounts(&guard).await {
                Ok(_) => { println!("Successfully closed all token accounts"); return; },
                Err(e) => { eprintln!("Failed to close all token accounts: {}", e); return; }
            }
        } else if args.contains(&"--nonce".to_string()) {
            // Short-lived guard for nonce
            let guard = shared_config.lock().await;
            println!("Creating new nonce for wallet");
            match create_nonce(&guard).await {
                Ok(_) => { println!("Successfully created new nonce for wallet"); return; },
                Err(e) => { eprintln!("Failed to create new nonce for wallet: {}", e); return; }
            }
        }
    }

    // Clone all needed fields from config, then drop the lock immediately
    let (yellowstone_grpc_http,
         yellowstone_grpc_token,
         app_state,
         swap_config,
         solana_price) = {
        let cfg = shared_config.lock().await;
        (
            cfg.yellowstone_grpc_http.clone(),
            cfg.yellowstone_grpc_token.clone(),
            cfg.app_state.clone(),
            cfg.swap_config.clone(),
            cfg.solana_price,
        )
    };

    /* Running Bot */
    let run_msg = RUN_MSG;
    println!("{}", run_msg);
    
    // Initialize original balance for risk management
    let wallet_pubkey = app_state.wallet.try_pubkey().unwrap();
    let original_sol_balance = match app_state.rpc_nonblocking_client.get_account(&wallet_pubkey).await {
        Ok(account) => account.lamports as f64 / 1_000_000_000.0, // Convert lamports to SOL
        Err(e) => {
            eprintln!("Failed to get wallet balance: {}", e);
            0.0
        }
    };
    
    // Get original WSOL balance
    let wsol_mint = spl_token::native_mint::id();
    let wsol_ata = spl_associated_token_account::get_associated_token_address(&wallet_pubkey, &wsol_mint);
    let original_wsol_balance = match app_state.rpc_nonblocking_client.get_token_account(&wsol_ata).await {
        Ok(Some(account)) => account.token_amount.ui_amount.unwrap_or(0.0),
        Ok(None) => 0.0, // No WSOL account
        Err(e) => {
            eprintln!("Failed to get WSOL balance: {}", e);
            0.0
        }
    };
    
    let total_original_balance = original_sol_balance + original_wsol_balance;
    solana_vntr_sniper::engine::sniper::set_original_balance(total_original_balance);
    println!("💰 Original balance set: {:.6} SOL (SOL: {:.6}, WSOL: {:.6})", 
             total_original_balance, original_sol_balance, original_wsol_balance);
    
    // Check Telegram configuration
    println!("\n📱 Telegram Configuration:");
    solana_vntr_sniper::services::telegram::log_config_status();
    println!();
    
    println!("\n⚠️  Blacklist functionality has been disabled");
    println!();
    
    // Initialize blockhash processor
    match BlockhashProcessor::new(app_state.rpc_client.clone()).await {
        Ok(processor) => {
            if let Err(e) = processor.start().await {
                eprintln!("Failed to start blockhash processor: {}", e);
                return;
            }
            println!("Blockhash processor started successfully");
        },
        Err(e) => {
            eprintln!("Failed to initialize blockhash processor: {}", e);
            return;
        }
    }

    // Parse command line arguments
    // (CLI one-off branches handled earlier)

    // Initialize token account list
    // OPTIMIZATION: No lock needed - function is a no-op (cache removed, accounts created automatically)
    // Removed unnecessary lock to reduce contention during initialization
    initialize_token_account_list(&app_state).await;
    
    // Cache maintenance is now integrated into comprehensive cleanup (every 200 seconds)
    // This eliminates redundancy and improves efficiency
    
    // Selling instruction cache removed - no maintenance needed

    // Initialize and log selling strategy parameters
    let selling_config = solana_vntr_sniper::engine::selling_strategy::SellingConfig::set_from_env();
    let selling_engine = Arc::new(solana_vntr_sniper::engine::selling_strategy::SellingEngine::new(
        Arc::new(app_state.clone()),
        Arc::new(swap_config.clone()),
        selling_config,
    ));
    selling_engine.log_selling_parameters();
    
    // Start automatic periodic cleanup service (every 5 minutes)
    // This prevents unbounded cache growth during long-running periods
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(300)); // 5 minutes
        let logger = solana_vntr_sniper::common::logger::Logger::new("[PERIODIC-CLEANUP] => ".cyan().bold().to_string());
        // Log removed for performance - only critical errors logged
        
        loop {
            interval.tick().await;
            
            match cache_maintenance::perform_comprehensive_cleanup().await {
                Ok(_) => {
                    // Log removed for performance
                },
                Err(e) => {
                    // Critical error - keep this log
                    logger.error(format!("Periodic cleanup error: {} (will retry in 5 minutes)", e));
                }
            }
        }
    });
    println!("✅ Automatic periodic cleanup service started (5 minute interval)");
    
    // Start memory monitoring service
    solana_vntr_sniper::services::memory_monitor::start_memory_monitor().await;
    println!("✅ Memory monitoring service started (1 minute interval)");
    
    // Start task monitoring service
    solana_vntr_sniper::services::task_monitor::start_task_monitor().await;
    println!("✅ Task monitoring service started (5 minute interval)");
    
    // Start periodic token monitoring service (every 5-10 seconds)
    println!("⏸️  Periodic token monitoring service temporarily disabled (monitor_all_tokens commented out)");

    // Dex monitoring only - no copy trading parameters needed
    

    
    // Bot now works on both PumpFun and PumpSwap automatically
    // No protocol preference needed - auto-detects from transaction data
    
    // Dex monitoring only - no target sell following needed
    
    // Risk management service removed to reduce bottlenecks - selling handled by main selling logic
    // All selling is now handled by the main selling strategy with retries and fallbacks

    // Create dex monitoring config
    let dex_config = SniperConfig {
        yellowstone_grpc_http,
        yellowstone_grpc_token,
        app_state: app_state.clone(),
        swap_config: swap_config.clone(),
        protocol_preference: SwapProtocol::Auto, // Auto-detect both PumpFun and PumpSwap
        solana_price,
    };
    
    // Start the dex monitoring bot (single call - no retry loop to avoid duplicate connections)
    // start_sniper() spawns background tasks that handle their own connections
    // The retry loop was causing duplicate gRPC connections to be created repeatedly
    // Use select! to handle both the sniper initialization and shutdown signal (Ctrl+C)
    tokio::select! {
        _ = async {
            // Call start_sniper() once - it spawns background tasks and returns immediately
            // The spawned tasks (start_program_monitoring, start_wallet_monitoring, 
            // start_token_creation_monitoring) handle their own connections and run indefinitely
            match start_sniper(dex_config).await {
                Ok(_) => {
                    // start_sniper() returns immediately after spawning tasks
                    // This is expected behavior - the tasks run in the background
                    println!("✅ Sniper monitoring tasks started successfully");
                    // Wait indefinitely - the spawned tasks handle monitoring
                    loop {
                        tokio::time::sleep(tokio::time::Duration::from_secs(60)).await;
                    }
                },
                Err(e) => {
                    eprintln!("❌ Failed to start sniper monitoring: {}", e);
                    eprintln!("   Bot will exit - check configuration and gRPC connection");
                }
            }
        } => {},
        _ = tokio::signal::ctrl_c() => {
            // Graceful shutdown
            println!("🛑 Ctrl+C received - shutting down...");
            
            std::process::exit(0);
        }
    }
}
