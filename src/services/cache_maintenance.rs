use std::time::Duration;
use std::sync::atomic::{AtomicUsize, Ordering};
use colored::Colorize;
use std::sync::Arc;

use crate::common::logger::Logger;
use crate::common::cache::{cleanup_old_price_drops, DEAD_TOKEN_LIST, cleanup_thresholds};
use crate::common::cache::TRADE_METRICS;
/// Guard struct to ensure cleanup lock is released when function exits
/// Similar to CleanupGuard in cache.rs, but for use in cache_maintenance.rs
struct TokenCleanupGuard<'a> {
    mint: &'a str,
}

impl<'a> Drop for TokenCleanupGuard<'a> {
    fn drop(&mut self) {
        use crate::common::cache::CLEANUP_IN_PROGRESS;
        CLEANUP_IN_PROGRESS.remove(self.mint);
    }
}

/// Reference counter to track how many cleanup operations are running (for metrics/logging only)
/// This is no longer used to pause monitoring - kept for potential future metrics
static CLEANUP_COUNT: AtomicUsize = AtomicUsize::new(0);

// REMOVED: MONITORING_PAUSED and related functions
// Monitoring is never paused globally. Fine-grained per-token locking (CLEANUP_IN_PROGRESS in cache.rs) 
// handles conflicts safely without blocking all transactions.

/// Increment cleanup counter (for metrics/logging only)
fn start_cleanup() {
    CLEANUP_COUNT.fetch_add(1, Ordering::AcqRel);
}

/// Decrement cleanup counter (for metrics/logging only)
fn finish_cleanup() {
    CLEANUP_COUNT.fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
        if count > 0 {
            Some(count - 1)
        } else {
            None // Prevent underflow
        }
    }).ok();
}

/// Perform cache cleanup operations (candles, dead tokens)
/// This is a shared function used by comprehensive cleanup
/// Note: Monitoring continues during cleanup - conflicts are handled via fine-grained per-token locking
async fn perform_cache_cleanup(logger: &Logger) {
    // Cleanup runs concurrently with monitoring - DashMaps are thread-safe
    // Fine-grained per-token locking (CLEANUP_IN_PROGRESS) prevents conflicts
    // Timeout of 30s for large datasets with batched processing
    let total_cleanup_start = std::time::Instant::now();
    
    let cleanup_result = tokio::time::timeout(
        Duration::from_secs(30),
        async {
            
            // Prune candles older than retention window (now async with timing)
            // Log removed - routine cleanup
            let candle_start = std::time::Instant::now();
            let now_secs = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            let retention_secs = cleanup_thresholds::CANDLE_RETENTION_SECS;
            let cutoff_ts = now_secs.saturating_sub(retention_secs);
            
            TRADE_METRICS.prune_candles_older_than(cutoff_ts).await;
            let candle_duration = candle_start.elapsed();
            // Log removed - routine cleanup
            
            // Clean up DEAD_TOKEN_LIST
            // OPTIMIZED: Use retain() for in-place removal instead of collect-then-remove
            // Log removed - routine cleanup
            let now_secs = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            let expiration_cutoff = now_secs.saturating_sub(cleanup_thresholds::DEAD_TOKEN_EXPIRATION_SECS);
            
            // Count items before removal
            let initial_count = DEAD_TOKEN_LIST.len();
            DEAD_TOKEN_LIST.retain(|_mint, &mut dead_timestamp| {
                dead_timestamp >= expiration_cutoff
            });
            let removed_count = initial_count - DEAD_TOKEN_LIST.len();
            
            if removed_count > 0 {
                // Log removed - routine cleanup
            } else {
                // Log removed - routine cleanup
            }
            
            // Cleanup old price drop records to prevent memory leaks
            // Log removed - routine cleanup
            let removed_drops_count = cleanup_old_price_drops(cleanup_thresholds::RECENT_PRICE_DROPS_RETENTION_SECS).await;
            
            if removed_drops_count > 0 {
                // Log removed - routine cleanup
            } else {
                // Log removed - routine cleanup
            }
            
            // Enforce cache size limits (prune if needed) with timing
            // Log removed - routine cleanup
            let limits_start = std::time::Instant::now();
            use crate::common::cache::enforce_cache_size_limits;
            enforce_cache_size_limits().await;
            let limits_duration = limits_start.elapsed();
            // Log removed - routine cleanup
            
            // Clean up stuck progress entries (operations that timed out) with timing
            // Log removed - routine cleanup
            let progress_start = std::time::Instant::now();
            use crate::common::cache::cleanup_stuck_progress_entries;
            let stuck_count = cleanup_stuck_progress_entries().await;
            let progress_duration = progress_start.elapsed();
            if stuck_count > 0 {
                // Log removed - routine cleanup
            } else {
                // Log removed - routine cleanup
            }
            
            Ok::<(), String>(())
        }
    ).await;
    
    let total_duration = total_cleanup_start.elapsed();
    
    match cleanup_result {
        Ok(Ok(_)) => {
            // Log removed - routine cleanup
        }
        Ok(Err(e)) => {
            // Log removed - error is handled
        }
        Err(_) => {
            // Log removed - timeout is handled
        }
    }
}

/// Comprehensive cleanup function that performs cache cleanup WITHOUT pausing monitoring
/// Uses fine-grained per-token locking to prevent conflicts with active operations
/// Note: Token selling is handled by the selling strategy, not by cache cleanup
pub async fn perform_comprehensive_cleanup() -> Result<(), String> {
    let logger = Logger::new("[COMPREHENSIVE-CLEANUP] => ".red().bold().to_string());
    
    // Log removed - routine cleanup
    
    // Increment cleanup counter (for metrics/logging only)
    start_cleanup();
    
    // Perform cache cleanup - monitoring continues during cleanup
    // Fine-grained per-token locking (CLEANUP_IN_PROGRESS) prevents conflicts
    // Cleanup automatically skips tokens that are actively being used
    // Log removed - routine cleanup
    perform_cache_cleanup(&logger).await;
    
    // Decrement cleanup counter
    finish_cleanup();
    
    // Log removed - routine cleanup
    
    Ok(())
}

// REMOVED: Regular comprehensive cleanup service
// Cache cleanup is now integrated into comprehensive cleanup
// This function is kept commented for reference but is no longer used
/*
/// Start regular comprehensive cleanup service (every 200 seconds)
pub async fn start_regular_comprehensive_cleanup() {
    let logger = Logger::new("[REGULAR-CLEANUP] => ".cyan().bold().to_string());
    // Log removed - service started
    
    tokio::spawn(async move {
        let mut interval = time::interval(Duration::from_secs(200));
        
        loop {
            interval.tick().await;
            
            // Log removed - routine cleanup
            
            if let Err(e) = perform_comprehensive_cleanup().await {
                // Log removed - error is handled
            }
        }
    });
}
*/

/// Trigger lightweight cleanup after successful sell (does NOT pause monitoring)
/// This performs cache cleanup without blocking the main process
pub fn trigger_lightweight_cleanup_after_sell(token_mint: &str) {
    let token_mint_clone = token_mint.to_string();
    
    // Perform lightweight cleanup in background without pausing monitoring
    tokio::spawn(async move {
        // Small delay to ensure locks are fully released before cleanup starts
        tokio::time::sleep(Duration::from_millis(100)).await;
        
        let logger = Logger::new(format!("[LIGHTWEIGHT-CLEANUP:{}] => ", token_mint_clone).yellow().to_string());
        // Log removed - routine cleanup
        
        // Perform cache cleanup WITHOUT pausing monitoring
        // This allows main process to continue independently
        let cleanup_start = std::time::Instant::now();
        perform_cache_cleanup(&logger).await;
        let cleanup_duration = cleanup_start.elapsed();
        
        // Log removed - routine cleanup
    });
}

/// Trigger cleanup after successful sell (DEPRECATED: pauses monitoring)
/// Use trigger_lightweight_cleanup_after_sell instead for non-blocking cleanup
pub fn trigger_cleanup_after_sell(token_mint: &str) {
    let token_mint_clone = token_mint.to_string();
    
    // Perform cleanup in background to avoid blocking the sell operation
    tokio::spawn(async move {
        // Small delay to ensure operations are fully completed before cleanup starts
        tokio::time::sleep(Duration::from_millis(100)).await;
        
        let logger = Logger::new(format!("[CLEANUP-AFTER-SELL:{}] => ", token_mint_clone).yellow().to_string());
        // Log removed - routine cleanup
        
        // Use timeout to prevent cleanup from blocking indefinitely
        match tokio::time::timeout(Duration::from_secs(10), perform_comprehensive_cleanup()).await {
            Ok(Ok(_)) => {
                // Log removed - routine cleanup
            },
            Ok(Err(e)) => {
                // Log removed - error is handled
            },
            Err(_) => {
                // Log removed - timeout is handled
            }
        }
    });
}
