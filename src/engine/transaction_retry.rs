use std::sync::Arc;
use std::str::FromStr;
use std::time::{Duration, Instant};
use anyhow::{anyhow, Result};
use anchor_client::solana_sdk::{
    pubkey::Pubkey, 
    signature::{Signature, Keypair}, 
    signer::Signer,
    hash::Hash,
};
use spl_associated_token_account::{get_associated_token_address, get_associated_token_address_with_program_id};
use colored::Colorize;
use tokio::time::{sleep, timeout};
use base64;

use crate::common::{
    config::{AppState, SwapConfig},
    logger::Logger,
};
use crate::engine::swap::SwapDirection;
use crate::services::jupiter_api::JupiterClient;
use crate::services::telegram;
use crate::engine::transaction_parser::TradeInfoFromToken;
use crate::core::tx;

/// Maximum number of retry attempts for selling transactions
const MAX_RETRIES: u32 = 3;

/// Result of a selling transaction attempt
#[derive(Debug)]
pub struct SellTransactionResult {
    pub success: bool,
    pub signature: Option<Signature>,
    pub error: Option<String>,
    pub used_jupiter_fallback: bool,
    pub attempt_count: u32,
}

/// Execute PumpFun sell transaction
async fn execute_pumpfun_sell(
    trade_info: &TradeInfoFromToken,
    sell_config: &SwapConfig,
    app_state: Arc<AppState>,
    logger: &Logger,
) -> Result<(Signature, f64, f64)> {
    // CRITICAL FIX: Double-check SELL_REASONS before building transaction
    use crate::engine::sniper::SELL_REASONS;
    if !SELL_REASONS.contains_key(&trade_info.mint) {
        return Err(anyhow!("Sell reason not set - skipping transaction building"));
    }
    
    logger.log("🚀 Executing PumpFun sell".purple().to_string());
    
    // Only proceed if this is a PumpFun token
    if trade_info.dex_type != crate::engine::transaction_parser::DexType::PumpFun {
        return Err(anyhow!("Not a PumpFun token"));
    }
    
    // CRITICAL: Set PROGRESS_ON_SELLING before calling selling transaction building function
    // This prevents race conditions where selling condition checks might interfere with transaction building
    use crate::common::cache::PROGRESS_ON_SELLING;
    PROGRESS_ON_SELLING.insert(trade_info.mint.clone(), ());
    
    // Create PumpFun instance
    let pump = crate::dex::pump_fun::Pump::new(
        app_state.rpc_nonblocking_client.clone(),
        app_state.rpc_client.clone(),
        app_state.wallet.clone(),
    );
    
    // Build swap instructions
    let (keypair, instructions, price) = pump.build_swap_from_parsed_data(trade_info, sell_config.clone()).await
        .map_err(|e| anyhow!("PumpFun build_swap_from_parsed_data failed: {}", e))?;
    
    // Get real-time blockhash
    let recent_blockhash = crate::services::blockhash_processor::BlockhashProcessor::get_latest_blockhash().await
        .ok_or_else(|| anyhow!("Failed to get real-time blockhash"))?;
    
    // Send transaction using zeroslot
    let signatures = tx::new_signed_and_send_zeroslot(
        app_state.zeroslot_rpc_client.clone(),
        recent_blockhash,
        &keypair,
        instructions,
        logger,
        false, // is_buy = false for selling
        None,  // slot = None for selling
    ).await.map_err(|e| anyhow!("PumpFun transaction send failed: {}", e))?;
    
    if signatures.is_empty() {
        return Err(anyhow!("No transaction signature returned"));
    }
    
    let signature = signatures[0].parse::<Signature>()
        .map_err(|e| anyhow!("Failed to parse signature: {}", e))?;
    
    // Calculate expected SOL received (approximate from price and amount)
    // For more accurate value, we'd need to query the transaction, but this is good enough for notification
    use crate::engine::sniper::TOKEN_HOLDINGS;
    let received_sol = if let Some(bought_info) = TOKEN_HOLDINGS.get(&trade_info.mint) {
        // Estimate: tokens_sold * price
        bought_info.current_amount * price
    } else {
        // Fallback: use price from build_swap
        price
    };
    
    logger.log(format!("✅ PumpFun transaction sent: {}", signature).green().to_string());
    Ok((signature, received_sol, price))
}

/// Execute normal sell (PumpFun) - NO RETRY LOGIC to prevent race conditions
async fn execute_normal_sell_with_retry(
    trade_info: &TradeInfoFromToken,
    sell_config: SwapConfig,
    app_state: Arc<AppState>,
    logger: &Logger,
) -> Result<SellTransactionResult> {
    // Only try PumpFun for PumpFun tokens
    if trade_info.dex_type != crate::engine::transaction_parser::DexType::PumpFun {
        return Err(anyhow!("Not a PumpFun token - skipping normal sell"));
    }
    
    logger.log(format!("🚀 PumpFun sell execution (single attempt, no retries) for token: {}", trade_info.mint).cyan().to_string());
    
    match execute_pumpfun_sell(trade_info, &sell_config, app_state.clone(), logger).await {
        Ok((signature, _received_sol, _price)) => {
            // Transaction sent successfully - wallet monitoring will handle confirmation
            // No RPC verification needed to reduce latency and bottleneck
            logger.log(format!("✅ PumpFun sell transaction sent: {} (wallet monitoring will confirm)", signature).green().to_string());
            Ok(SellTransactionResult {
                success: true,
                signature: Some(signature),
                error: None,
                used_jupiter_fallback: false,
                attempt_count: 1,
            })
        }
        Err(e) => {
            logger.log(format!("❌ PumpFun sell failed: {}", e).yellow().to_string());
            Err(anyhow!("PumpFun sell failed: {}", e))
        }
    }
}

/// Execute Jupiter fallback sell
async fn execute_jupiter_fallback_sell(
    trade_info: &TradeInfoFromToken,
    sell_config: &SwapConfig,
    app_state: Arc<AppState>,
    logger: &Logger,
) -> Result<Signature> {
    let (signature, _received_sol, _price) = execute_jupiter_sell(trade_info, sell_config, app_state, logger).await?;
    Ok(signature)
}

/// Execute a selling transaction with retry and Jupiter fallback
pub async fn execute_sell_with_retry_and_fallback(
    trade_info: &TradeInfoFromToken,
    sell_config: SwapConfig,
    app_state: Arc<AppState>,
    logger: &Logger,
) -> Result<SellTransactionResult> {
    let token_mint = &trade_info.mint;
    logger.log(format!("🔄 Starting sell transaction with retry for token: {}", token_mint).cyan().to_string());

    // First, try the normal selling flow with retries
    match execute_normal_sell_with_retry(trade_info, sell_config.clone(), app_state.clone(), logger).await {
        Ok(result) => {
            if result.success {
                logger.log(format!("✅ Normal sell succeeded on attempt {} - wallet monitoring will send telegram notification", result.attempt_count).green().to_string());
                
                // Don't remove SELL_REASONS here - wallet monitoring will handle notification and cleanup
                // This ensures wallet monitoring has access to sell reason when it detects the balance change
                
                return Ok(result);
            }
        }
        Err(e) => {
            logger.log(format!("❌ Normal sell attempts failed: {}", e).yellow().to_string());
        }
    }

    // If normal selling failed after retries, try Jupiter fallback
    logger.log(format!("🚀 Attempting Jupiter API fallback for token: {}", token_mint).purple().to_string());
    
    match execute_jupiter_fallback_sell(trade_info, &sell_config, app_state.clone(), logger).await {
        Ok(signature) => {
            logger.log(format!("✅ Jupiter fallback sell succeeded: {} - wallet monitoring will send telegram notification", signature).green().to_string());
            
            // Don't remove SELL_REASONS here - wallet monitoring will handle notification and cleanup
            // This ensures wallet monitoring has access to sell reason when it detects the balance change
            
            Ok(SellTransactionResult {
                success: true,
                signature: Some(signature),
                error: None,
                used_jupiter_fallback: true,
                attempt_count: MAX_RETRIES + 1,
            })
        }
        Err(e) => {
            logger.log(format!("❌ Jupiter fallback sell failed: {}", e).red().to_string());
            Ok(SellTransactionResult {
                success: false,
                signature: None,
                error: Some(format!("All sell attempts failed. Last error: {}", e)),
                used_jupiter_fallback: true,
                attempt_count: MAX_RETRIES + 1,
            })
        }
    }
}

/// Execute Jupiter API sell (unified selling method for all tokens)
/// Returns (signature, received_sol, price) for notification
async fn execute_jupiter_sell(
    trade_info: &TradeInfoFromToken,
    sell_config: &SwapConfig,
    app_state: Arc<AppState>,
    logger: &Logger,
) -> Result<(Signature, f64, f64)> {
    // CRITICAL FIX: Double-check SELL_REASONS before building transaction
    // This is a safety net in case the check in execute_sell_with_retry_and_fallback was bypassed
    use crate::engine::sniper::SELL_REASONS;
    if !SELL_REASONS.contains_key(&trade_info.mint) {
        return Err(anyhow!("Sell reason not set - skipping transaction building"));
    }
    
    logger.log("🚀 Executing Jupiter API sell (unified system)".purple().to_string());

    // Get wallet pubkey
    let wallet_pubkey = app_state.wallet.try_pubkey()
        .map_err(|e| anyhow!("Failed to get wallet pubkey: {}", e))?;

    // Get token mint pubkey
    let token_pubkey = trade_info.mint.parse::<Pubkey>()
        .map_err(|e| anyhow!("Invalid token mint address: {}", e))?;

    // OPTIMIZATION: Always use Token-2022 program (removed RPC call for performance)
    let token_program_id = Pubkey::from_str("TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb")
        .map_err(|e| anyhow!("Failed to parse Token-2022 program ID: {}", e))?;

    // OPTIMIZATION: Prefer TOKEN_HOLDINGS.current_amount, only fetch from RPC if not found (single RPC call)
    use crate::engine::sniper::TOKEN_HOLDINGS;
    let token_amount = if let Some(bought_info) = TOKEN_HOLDINGS.get(&trade_info.mint) {
        // Use cached amount from TOKEN_HOLDINGS (no RPC call)
        const DECIMALS: u8 = 6;
        (bought_info.current_amount * 10f64.powi(DECIMALS as i32)) as u64
    } else {
        // Only fetch from RPC if not in TOKEN_HOLDINGS (single RPC call in retry logic)
        // Get associated token account
        let ata = get_associated_token_address_with_program_id(&wallet_pubkey, &token_pubkey, &token_program_id);
        
        // Get current token balance from RPC (only time we fetch in retry logic)
        let token_account = app_state.rpc_nonblocking_client.get_token_account(&ata).await
            .map_err(|e| anyhow!("Failed to get token account: {}", e))?
            .ok_or_else(|| anyhow!("Token account not found"))?;

        token_account.token_amount.amount.parse::<u64>()
            .map_err(|e| anyhow!("Failed to parse token amount: {}", e))?
    };

    if token_amount == 0 {
        return Err(anyhow!("No tokens to sell"));
    }

    // Apply sell percentage based on amount_in field (which represents percentage for sells)
    let amount_to_sell = if sell_config.amount_in >= 1.0 {
        token_amount
    } else {
        ((token_amount as f64) * sell_config.amount_in) as u64
    };

    logger.log(format!("💱 Selling {} tokens via Jupiter API", amount_to_sell));

    // OPTIMIZATION: Use shared JupiterClient from AppState (eliminates duplicate initialization)
    // Get quote first to calculate expected SOL output
    const SOL_MINT: &str = "So11111111111111111111111111111111111111112";
    // Use 15000 bps (150%) slippage to accept any output amount (equivalent to setting output to 0 or 1)
    const SELL_SLIPPAGE_ACCEPT_ANY: u64 = 15000; // 150% slippage = accept any output
    let quote = app_state.jupiter_client.get_quote(
        &trade_info.mint,
        SOL_MINT,
        amount_to_sell,
        SELL_SLIPPAGE_ACCEPT_ANY,
    ).await.map_err(|e| anyhow!("Jupiter quote failed: {}", e))?;

    // Calculate expected SOL output
    let expected_sol_raw = quote.out_amount.parse::<u64>()
        .map_err(|e| anyhow!("Failed to parse output amount: {}", e))?;
    let expected_sol = expected_sol_raw as f64 / 1e9;

    // Skip if expected output is too small
    if expected_sol < 0.0001 {
        return Err(anyhow!("Expected SOL output too small: {} SOL", expected_sol));
    }

    logger.log(format!("💰 Expected SOL from sale: {:.6}", expected_sol));

    // Execute sell transaction via Jupiter API (this handles signing and sending)
    let signature_str = app_state.jupiter_client.sell_token_with_jupiter(
        &trade_info.mint,
        amount_to_sell,
        15000, // 150% slippage = accept any output
        &app_state.wallet,
    ).await.map_err(|e| anyhow!("Jupiter API sell failed: {}", e))?;
    
    // Parse the signature string into a Signature type
    let signature = signature_str.parse::<anchor_client::solana_sdk::signature::Signature>()
        .map_err(|e| anyhow!("Failed to parse signature: {}", e))?;

    logger.log(format!("✅ Jupiter transaction sent: {}", signature).green().to_string());

    // Calculate price from quote (price per token)
    let price = if amount_to_sell > 0 {
        expected_sol / (amount_to_sell as f64 / 1e6) // Convert to price per token (assuming 6 decimals)
    } else {
        trade_info.post_current_price // Fallback to trade_info price
    };

    // Don't wait for verification - just return the signature with expected values
    // Verification can happen in background if needed, but shouldn't block selling
    Ok((signature, expected_sol, price))
} 