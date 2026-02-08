use std::time::Duration;
use colored::Colorize;
use crate::common::logger::Logger;
use crate::common::cache::{
    PROGRESS_ON_BUYING, TRADE_METRICS
};
use crate::engine::sniper::TOKEN_HOLDINGS;

/// Memory monitoring service that tracks cache sizes and alerts when approaching limits
/// Runs every 60 seconds and logs cache statistics
pub async fn start_memory_monitor() {
    tokio::spawn(async {
        let mut interval = tokio::time::interval(Duration::from_secs(60));
        let logger = Logger::new("[MEMORY-MONITOR] => ".magenta().bold().to_string());
        
        // Log removed for performance - only critical warnings logged
        
        loop {
            interval.tick().await;
            
            // Collect cache statistics
            let candle_count = TRADE_METRICS.total_candle_count();
            let progress_buying = PROGRESS_ON_BUYING.len();
            
            // Check for warnings - monitor candle count instead of creator records
            const MAX_CANDLES_WARNING: usize = 80_000;
            const MAX_CANDLES_CRITICAL: usize = 100_000;
            
            if candle_count >= MAX_CANDLES_WARNING {
                logger.critical(format!("WARNING: Candle count at {} (approaching limit)", candle_count));
            }
            
            // Critical alerts
            if candle_count >= MAX_CANDLES_CRITICAL {
                logger.critical(format!("CRITICAL: Candle count exceeded limit! ({})", candle_count));
                
                send_telegram_alert(&format!(
                    "🚨 CRITICAL: Candle count exceeded limit! ({})",
                    candle_count
                )).await;
            }
            
            // Alert for stuck progress entries (only if very high)
            if progress_buying > 50 {
                logger.critical(format!("WARNING: High in-progress operations (buying: {})", progress_buying));
            }
        }
    });
}

/// Send Telegram alert for critical cache issues (if Telegram is configured)
async fn send_telegram_alert(message: &str) {
    // Try to send Telegram notification (silently fails if not configured)
    crate::services::telegram::send_message_async(message.to_string()).await;
}

