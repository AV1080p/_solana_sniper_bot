use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use solana_sdk::hash::Hash;
use solana_sdk::pubkey::Pubkey;
use solana_client::rpc_client::RpcClient;
use anyhow::{Result, anyhow};
use colored::Colorize;
use lazy_static::lazy_static;
use std::str::FromStr;
use crate::common::logger::Logger;

// Global state for latest blockhash and timestamp (for command mode: --sell, --close, --nonce)
lazy_static! {
    static ref LATEST_BLOCKHASH: Arc<RwLock<Option<Hash>>> = Arc::new(RwLock::new(None));
    static ref BLOCKHASH_LAST_UPDATED: Arc<RwLock<Option<Instant>>> = Arc::new(RwLock::new(None));
}

// Global state for offchain blockhash from nonce account (for normal bot mode)
lazy_static! {
    static ref OFFCHAIN_BLOCKHASH: Arc<RwLock<Option<Hash>>> = Arc::new(RwLock::new(None));
}

const BLOCKHASH_STALENESS_THRESHOLD: Duration = Duration::from_secs(10);
const UPDATE_INTERVAL: Duration = Duration::from_millis(300);

pub struct BlockhashProcessor {
    rpc_client: Arc<RpcClient>,
    logger: Logger,
}

impl BlockhashProcessor {
    pub async fn new(rpc_client: Arc<RpcClient>) -> Result<Self> {
        let logger = Logger::new("[BLOCKHASH-PROCESSOR] => ".cyan().to_string());
        
        Ok(Self {
            rpc_client,
            logger,
        })
    }

    pub async fn start(&self) -> Result<()> {
        self.logger.log("Starting blockhash processor...".green().to_string());

        // Clone necessary components for the background task
        let rpc_client = self.rpc_client.clone();
        let logger = self.logger.clone();

        tokio::spawn(async move {
            loop {
                match Self::update_blockhash_from_rpc(&rpc_client).await {
                    Ok(blockhash) => {
                        // Update global blockhash
                        let mut latest = LATEST_BLOCKHASH.write().await;
                        *latest = Some(blockhash);
                        
                        // Update timestamp
                        let mut last_updated = BLOCKHASH_LAST_UPDATED.write().await;
                        *last_updated = Some(Instant::now());
                        
                        // logger.log(format!("Updated latest blockhash: {}", blockhash));
                    }
                    Err(e) => {
                        logger.log(format!("Error getting latest blockhash: {}", e).red().to_string());
                    }
                }

                tokio::time::sleep(UPDATE_INTERVAL).await;
            }
        });

        Ok(())
    }

    async fn update_blockhash_from_rpc(rpc_client: &RpcClient) -> Result<Hash> {
        rpc_client.get_latest_blockhash()
            .map_err(|e| anyhow!("Failed to get blockhash from RPC: {}", e))
    }

    /// Update the latest blockhash and its timestamp
    async fn update_blockhash(hash: Hash) {
        let mut latest = LATEST_BLOCKHASH.write().await;
        *latest = Some(hash);
        
        let mut last_updated = BLOCKHASH_LAST_UPDATED.write().await;
        *last_updated = Some(Instant::now());
    }

    /// Get the latest cached blockhash with freshness check
    pub async fn get_latest_blockhash() -> Option<Hash> {
        // Check if blockhash is stale
        let last_updated = BLOCKHASH_LAST_UPDATED.read().await;
        if let Some(instant) = *last_updated {
            if instant.elapsed() > BLOCKHASH_STALENESS_THRESHOLD {
                return None;
            }
        }
        
        let latest = LATEST_BLOCKHASH.read().await;
        *latest
    }

    /// Get a fresh blockhash, falling back to RPC if necessary
    pub async fn get_fresh_blockhash(&self) -> Result<Hash> {
        if let Some(hash) = Self::get_latest_blockhash().await {
            return Ok(hash);
        }
        
        // Fallback to RPC if cached blockhash is stale or missing
        self.logger.log("Cached blockhash is stale or missing, falling back to RPC...".yellow().to_string());
        let new_hash = self.rpc_client.get_latest_blockhash()
            .map_err(|e| anyhow!("Failed to get blockhash from RPC: {}", e))?;
        
        Self::update_blockhash(new_hash).await;
        Ok(new_hash)
    }

    /// Get offchain blockhash from nonce account
    /// This should be called when onchain state is updated (after buy/sell)
    pub async fn get_offchain_blockhash(&self) -> Result<Hash> {
        // Check if we have a cached offchain blockhash
        let cached = OFFCHAIN_BLOCKHASH.read().await;
        if let Some(hash) = *cached {
            return Ok(hash);
        }
        drop(cached);

        // Fetch from nonce account
        self.update_offchain_blockhash().await
    }

    /// Update offchain blockhash from nonce account
    /// This should be called:
    /// - When bot starts
    /// - After buying
    /// - After selling
    pub async fn update_offchain_blockhash(&self) -> Result<Hash> {
        let nonce_account_str = std::env::var("NONCE_ACCOUNT")
            .map_err(|_| anyhow!("NONCE_ACCOUNT environment variable not set"))?;
        
        let nonce_pubkey = Pubkey::from_str(&nonce_account_str)
            .map_err(|e| anyhow!("Invalid NONCE_ACCOUNT pubkey: {}", e))?;

        // Get nonce account data
        let nonce_account = self.rpc_client.get_account(&nonce_pubkey)
            .map_err(|e| anyhow!("Failed to get nonce account: {}", e))?;

        // Parse nonce data to get blockhash
        let nonce_data = solana_rpc_client_nonce_utils::data_from_account(&nonce_account)
            .map_err(|e| anyhow!("Failed to parse nonce data: {}", e))?;
        
        let offchain_blockhash = nonce_data.blockhash();

        // Cache the offchain blockhash
        let mut cached = OFFCHAIN_BLOCKHASH.write().await;
        *cached = Some(offchain_blockhash);
        
        self.logger.log(format!("Updated offchain blockhash from nonce account: {}", offchain_blockhash).green().to_string());
        
        Ok(offchain_blockhash)
    }

    /// Check if offchain blockhash is available (nonce account is configured)
    pub fn is_offchain_blockhash_available() -> bool {
        std::env::var("NONCE_ACCOUNT").is_ok()
    }

    /// Get blockhash based on mode: offchain for normal bot mode, recent for command mode
    pub async fn get_blockhash_for_transaction(&self, use_offchain: bool) -> Result<Hash> {
        if use_offchain && Self::is_offchain_blockhash_available() {
            self.get_offchain_blockhash().await
        } else {
            self.get_fresh_blockhash().await
        }
    }

    /// Check if we're in command mode (--sell, --close, --nonce, --wrap, --unwrap)
    pub fn is_command_mode() -> bool {
        let args: Vec<String> = std::env::args().collect();
        args.contains(&"--sell".to_string()) ||
        args.contains(&"--close".to_string()) ||
        args.contains(&"--nonce".to_string()) ||
        args.contains(&"--wrap".to_string()) ||
        args.contains(&"--unwrap".to_string())
    }

    /// Determine if we should use offchain blockhash (normal bot mode) or recent blockhash (command mode)
    pub fn should_use_offchain_blockhash() -> bool {
        !Self::is_command_mode() && Self::is_offchain_blockhash_available()
    }

    /// Get blockhash for transaction (static method that can be called without instance)
    /// Uses offchain blockhash in normal bot mode, recent blockhash in command mode
    pub async fn get_blockhash_for_transaction_static(rpc_client: Option<Arc<RpcClient>>) -> Result<Hash> {
        let use_offchain = Self::should_use_offchain_blockhash();
        
        if use_offchain {
            // Try to get cached offchain blockhash first
            let cached = OFFCHAIN_BLOCKHASH.read().await;
            if let Some(hash) = *cached {
                return Ok(hash);
            }
            drop(cached);

            // If not cached, we need RPC client to fetch from nonce account
            if let Some(client) = rpc_client {
                let processor = Self::new(client).await?;
                return processor.get_offchain_blockhash().await;
            } else {
                return Err(anyhow!("RPC client required to fetch offchain blockhash"));
            }
        } else {
            // Command mode: use recent blockhash
            if let Some(hash) = Self::get_latest_blockhash().await {
                return Ok(hash);
            }
            
            // Fallback to RPC if needed
            if let Some(client) = rpc_client {
                let processor = Self::new(client).await?;
                return processor.get_fresh_blockhash().await;
            } else {
                return Err(anyhow!("Failed to get blockhash: no cached value and no RPC client"));
            }
        }
    }
} 