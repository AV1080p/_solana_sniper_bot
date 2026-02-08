use once_cell::sync::Lazy;
use teloxide::prelude::*;
use std::env;

static BOT_TOKEN: Lazy<Option<String>> = Lazy::new(|| env::var("TELEGRAM_BOT_TOKEN").ok());
static CHAT_ID: Lazy<Option<i64>> = Lazy::new(|| env::var("TELEGRAM_CHAT_ID").ok().and_then(|v| v.parse::<i64>().ok()));

pub async fn send_message_async(text: String) -> Result<(), String> {
    let Some(token) = BOT_TOKEN.clone() else {
        let err = "TELEGRAM_BOT_TOKEN not configured".to_string();
        // Log removed for performance
        return Err(err);
    };
    let Some(chat_id) = CHAT_ID.clone() else {
        let err = "TELEGRAM_CHAT_ID not configured".to_string();
        // Log removed for performance
        return Err(err);
    };
    
    let bot = Bot::new(token);
    
    match bot.send_message(ChatId(chat_id), text).await {
        Ok(_) => Ok(()),
        Err(e) => {
            let err_msg = format!("Failed to send message: {}", e);
            // Critical error - keep this log
            eprintln!("[TELEGRAM] {}", err_msg);
            Err(err_msg)
        }
    }
}

/// Send message with retry logic
pub async fn send_message_with_retry(text: String, max_retries: u32) -> Result<(), String> {
    let mut last_error = None;
    
    for attempt in 1..=max_retries {
        match send_message_async(text.clone()).await {
            Ok(_) => {
                if attempt > 1 {
                    // Log removed for performance
                }
                return Ok(());
            }
            Err(e) => {
                last_error = Some(e.clone());
                if attempt < max_retries {
                    let delay_ms = 500 * attempt; // Exponential backoff: 500ms, 1000ms, 1500ms...
                    // Log removed for performance
                    tokio::time::sleep(tokio::time::Duration::from_millis(delay_ms as u64)).await;
                }
            }
        }
    }
    
    Err(last_error.unwrap_or_else(|| "All retry attempts failed".to_string()))
}

pub fn format_sell_message(mint: &str, received_sol: f64, price: f64, reason: &str, signature: &str, protocol: &str, token_age_secs: Option<u64>) -> String {
    // Token age removed to reduce reading of edge_price
    let age_info = String::new();
    
    // TEMPORARILY COMMENTED OUT: Holder count logic is not reasonable currently and causes race conditions
    // Get token holder count
    // use crate::common::cache::TRADE_METRICS;
    // let holder_count_info = TRADE_METRICS.get_token_holder_count(mint)
    //     .map(|count| format!("\n👥 Token Holders: {}", count))
    //     .unwrap_or_default();
    let holder_count_info = String::new();
    
    // Add protocol-specific emojis
    let protocol_emoji = match protocol {
        "PumpFun" => "🚀",
        "PumpSwap" => "💧",
        _ => "🔗",
    };
    
    format!(
        "🔴 SELL ORDER EXECUTED\n\n🪙 Mint: {}\n{} Protocol: {}\n💵 Received: {:.6} SOL\n💎 Price: {:.12} SOL/token\n📝 Reason: {}{}{}\n🔗 Tx: {}",
        mint, protocol_emoji, protocol, received_sol, price, reason, age_info, holder_count_info, signature
    )
}

pub fn format_buy_message(mint: &str, spent_sol: f64, price: f64, reason: &str, signature: &str, protocol: &str, token_amount: f64, token_age_secs: Option<u64>) -> String {
    // Token age removed to reduce reading of edge_price
    let age_info = String::new();
    
    // TEMPORARILY COMMENTED OUT: Holder count logic is not reasonable currently and causes race conditions
    // Get token holder count
    // use crate::common::cache::TRADE_METRICS;
    // let holder_count_info = TRADE_METRICS.get_token_holder_count(mint)
    //     .map(|count| format!("\n👥 Token Holders: {}", count))
    //     .unwrap_or_default();
    let holder_count_info = String::new();
    
    // Add protocol-specific emojis
    let protocol_emoji = match protocol {
        "PumpFun" => "🚀",
        "PumpSwap" => "💧",
        _ => "🔗",
    };
    
    // Token state removed - no longer tracked
    let state_info = String::new();
    
    // Format buying reason more clearly
    // If reason contains a colon, extract the type and details separately
    let (buying_type, buying_details) = if let Some(colon_pos) = reason.find(':') {
        let type_part = reason[..colon_pos].trim();
        let details_part = reason[colon_pos + 1..].trim();
        // If details are empty or just whitespace, use the type as the full reason
        if details_part.is_empty() {
            (type_part, "")
        } else {
            (type_part, details_part)
        }
    } else {
        // No colon found, use the whole reason as the type
        (reason, "")
    };
    
    // Format the reason section
    let reason_section = if !buying_details.is_empty() {
        format!("📝 Reason: {}: {}", buying_type, buying_details)
    } else {
        format!("📝 Reason: {}", buying_type)
    };
    
    format!(
        "🟢 BUY ORDER EXECUTED\n\n🪙 Mint: {}\n{} Protocol: {}\n💵 Spent: {:.6} SOL\n💎 Price: {:.12} SOL/token\n📊 Amount: {:.6} tokens{}\n{}{}\n🔗 Tx: {}{}",
        mint, protocol_emoji, protocol, spent_sol, price, token_amount, state_info, reason_section, holder_count_info, signature, age_info
    )
}

/// Check if Telegram is properly configured
pub fn is_configured() -> bool {
    BOT_TOKEN.is_some() && CHAT_ID.is_some()
}

/// Log Telegram configuration status
pub fn log_config_status() {
    if BOT_TOKEN.is_some() {
        // Log removed - configuration check
    } else {
        // Log removed - configuration check
    }
    
    if CHAT_ID.is_some() {
        // Log removed - configuration check
    } else {
        // Log removed - configuration check
    }
    
    if is_configured() {
        // Log removed - configuration check
    } else {
        // Log removed - configuration check
    }
}
