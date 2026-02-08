use std::time::{Duration, Instant};
use colored::Colorize;
use dashmap::DashMap;
use once_cell::sync::Lazy;
use crate::common::logger::Logger;

/// Global task registry to track spawned tasks and detect zombies
/// Maps task_id -> (start_time, description)
pub static ACTIVE_TASKS: Lazy<DashMap<String, (Instant, String)>> = Lazy::new(|| DashMap::new());

/// Register a task when it starts
/// task_id should be unique (e.g., "buy-{mint}", "sell-{mint}")
pub fn register_task(task_id: String, description: String) {
    ACTIVE_TASKS.insert(task_id, (Instant::now(), description));
}

/// Unregister a task when it completes
pub fn unregister_task(task_id: &str) {
    ACTIVE_TASKS.remove(task_id);
}

/// Get the number of active tasks
pub fn active_task_count() -> usize {
    ACTIVE_TASKS.len()
}

/// Task monitoring service that detects zombie tasks (running too long)
/// Runs every 5 minutes and logs warnings for tasks running > 10 minutes
pub async fn start_task_monitor() {
    tokio::spawn(async {
        let mut interval = tokio::time::interval(Duration::from_secs(300)); // 5 minutes
        let logger = Logger::new("[TASK-MONITOR] => ".cyan().bold().to_string());
        
        // Log removed for performance - only zombie tasks logged
        
        loop {
            interval.tick().await;
            
            let zombie_threshold = Duration::from_secs(600); // 10 minutes
            
            let mut zombie_tasks = Vec::new();
            
            // Check all active tasks - only track zombies
            for entry in ACTIVE_TASKS.iter() {
                let (task_id, (start_time, description)) = (entry.key(), entry.value());
                let elapsed = start_time.elapsed();
                
                if elapsed > zombie_threshold {
                    zombie_tasks.push((task_id.clone(), elapsed, description.clone()));
                }
            }
            
            // Report zombies only (critical)
            if !zombie_tasks.is_empty() {
                logger.critical(format!("{} ZOMBIE task(s) detected (running > 10 minutes):", zombie_tasks.len()));
                
                for (task_id, elapsed, description) in &zombie_tasks {
                    logger.critical(format!("   - {} ({:.1}m): {}", task_id, elapsed.as_secs_f64() / 60.0, description));
                }
                
                // Send Telegram alert for zombies
                if zombie_tasks.len() > 0 {
                    let message = format!(
                        "🚨 {} zombie task(s) detected running > 10 minutes",
                        zombie_tasks.len()
                    );
                    send_telegram_alert(&message).await;
                }
                
                // Auto-cleanup zombie tasks from registry (they're clearly stuck)
                for (task_id, _, _) in zombie_tasks {
                    ACTIVE_TASKS.remove(&task_id);
                }
            }
        }
    });
}

/// Send Telegram alert for critical task issues (if Telegram is configured)
async fn send_telegram_alert(message: &str) {
    // Try to send Telegram notification (silently fails if not configured)
    crate::services::telegram::send_message_async(message.to_string()).await;
}

