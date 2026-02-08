use anyhow::Result;
use colored::Colorize;
use dotenv::dotenv;
use reqwest::Error;
use serde::Deserialize;
use anchor_client::solana_sdk::{commitment_config::CommitmentConfig, signature::Keypair, signer::Signer};
use tokio::sync::{Mutex, OnceCell};
use std::{env, sync::Arc};
use crate::engine::swap::SwapProtocol;
use crate::{
    common::{constants::INIT_MSG, logger::Logger},
    engine::swap::{SwapDirection, SwapInType},
    services::jupiter_api::JupiterClient,
};
use std::time::Duration;

static GLOBAL_CONFIG: OnceCell<Mutex<Config>> = OnceCell::const_new();


pub struct Config {
    pub yellowstone_grpc_http: String,
    pub yellowstone_grpc_token: String,
    pub app_state: AppState,
    pub swap_config: SwapConfig,
    pub zero_slot_tip_value: f64, // New: Tip value for zeroslot selling
    pub solana_price: f64,
}

impl Config {
    pub async fn new() -> &'static Mutex<Config> {
        GLOBAL_CONFIG
            .get_or_init(|| async {
            let init_msg = INIT_MSG;
            // Log removed - initialization message

            dotenv().ok(); // Load .env file

            let logger = Logger::new("[INIT] => ".blue().bold().to_string());

            let yellowstone_grpc_http = import_env_var("YELLOWSTONE_GRPC_HTTP");
            let yellowstone_grpc_token = import_env_var("YELLOWSTONE_GRPC_TOKEN");
            
            // Read buy slippage only (selling uses 0 or 1 for output amount to ensure it works)
            let buy_slippage_input = import_env_var("BUY_SLIPPAGE").parse::<u64>().unwrap_or(700);
            
            // Allow much higher slippage for buys (up to 50000 bps = 500%)
            let max_slippage: u64 = 50000;
            let buy_slippage = if buy_slippage_input > max_slippage {
                max_slippage
            } else {
                buy_slippage_input
            };
            
            logger.log(format!("💰 Buy slippage: {} bps ({}%)", 
                buy_slippage, buy_slippage as f64 / 100.0).cyan().to_string());
            
            // Read selling configuration for front-running
            let zero_slot_tip_value = import_env_var("ZERO_SLOT_TIP_VALUE").parse::<f64>().unwrap_or(0.0025);
            
            let solana_price = create_coingecko_proxy().await.unwrap_or(200_f64);
            let _rpc_client = create_rpc_client().unwrap();
            let rpc_nonblocking_client = create_nonblocking_rpc_client().await.unwrap();
            let zeroslot_rpc_client = create_zeroslot_rpc_client().await.unwrap();
            let wallet: std::sync::Arc<anchor_client::solana_sdk::signature::Keypair> = import_wallet().unwrap();
            let balance = match rpc_nonblocking_client
                .get_account(&wallet.pubkey())
                .await {
                    Ok(account) => account.lamports,
                    Err(err) => {
                        logger.log(format!("Failed to get wallet balance: {}", err).red().to_string());
                        0 // Default to zero if we can't get the balance
                    }
                };

            let wallet_cloned = wallet.clone();
            let swap_direction = SwapDirection::Buy; //SwapDirection::Sell
            let in_type = SwapInType::Qty; //SwapInType::Pct
            let amount_in = import_env_var("BUY_AMOUNT_IN_SOL")
                .parse::<f64>()
                .unwrap_or(0.001_f64); //quantity
                                        // let in_type = "pct"; //percentage
                                        // let amount_in = 0.5; //percentage

            let swap_config = SwapConfig {
                swap_direction,
                in_type,
                amount_in,
                buy_slippage,
                reverse: false, // Default to normal mode
            };

            let rpc_client = create_rpc_client().unwrap();
            // OPTIMIZATION: Initialize JupiterClient once and reuse (eliminates 3+ initializations per sell)
            let jupiter_client = Arc::new(JupiterClient::new(rpc_nonblocking_client.clone()));
            let app_state = AppState {
                rpc_client,
                rpc_nonblocking_client,
                zeroslot_rpc_client,
                wallet,
                protocol_preference: SwapProtocol::default(),
                jupiter_client,
            };
           logger.log(
                    format!(
                    "[SNIPER ENVIRONMENT]: \n\t\t\t\t [Yellowstone gRpc]: {},
                    \n\t\t\t\t * [Wallet]: {:?}, * [Balance]: {} Sol, 
                    \n\t\t\t\t * [Buy Slippage]: {} bps, * [Solana]: {}, * [Amount]: {}",
                    yellowstone_grpc_http,
                    wallet_cloned.pubkey(),
                    balance as f64 / 1_000_000_000_f64,
                    buy_slippage,
                    solana_price,
                    amount_in,
                )
                .purple()
                .italic()
                .to_string(),
            );
            Mutex::new(Config {
                yellowstone_grpc_http,
                yellowstone_grpc_token,
                app_state,
                swap_config,
                zero_slot_tip_value,
                solana_price,
            })
        })
        .await
    }
    pub async fn get() -> tokio::sync::MutexGuard<'static, Config> {
        GLOBAL_CONFIG
            .get()
            .expect("Config not initialized")
            .lock()
            .await
    }
}

//pumpfun
pub const LOG_INSTRUCTION: &str = "initialize2";
pub const PUMP_LOG_INSTRUCTION: &str = "MintTo";
pub const PUMP_FUN_BUY_LOG_INSTRUCTION: &str = "Buy";
pub const PUMP_FUN_PROGRAM_DATA_PREFIX: &str = "Program data: G3KpTd7rY3Y";
pub const PUMP_FUN_SELL_LOG_INSTRUCTION: &str = "Sell";
pub const PUMP_FUN_BUY_OR_SELL_PROGRAM_DATA_PREFIX: &str = "Program data: vdt/007mYe";

//TODO: pumpswap
pub const PUMP_SWAP_LOG_INSTRUCTION: &str = "Migerate";
pub const PUMP_SWAP_BUY_LOG_INSTRUCTION: &str = "Buy";
pub const PUMP_SWAP_BUY_PROGRAM_DATA_PREFIX: &str = "PProgram data: Z/RSHyz1d3";
pub const PUMP_SWAP_SELL_LOG_INSTRUCTION: &str = "Sell";
pub const PUMP_SWAP_SELL_PROGRAM_DATA_PREFIX: &str = "Program data: Pi83CqUD3Cp";

use std::cmp::Eq;
use std::hash::{Hash, Hasher};

#[derive(Debug, PartialEq, Clone)]
pub struct LiquidityPool {
    pub mint: String,
    pub buy_price: f64,
    pub sell_price: f64,
    pub status: Status,
    pub timestamp: Option<tokio::time::Instant>,
}

impl Eq for LiquidityPool {}
impl Hash for LiquidityPool {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.mint.hash(state);
        self.buy_price.to_bits().hash(state); // Convert f64 to bits for hashing
        self.sell_price.to_bits().hash(state);
        self.status.hash(state);
    }
}

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub enum Status {
    Bought,
    Buying,
    Checking,
    Sold,
    Selling,
    Failure,
}

#[derive(Deserialize)]
struct CoinGeckoResponse {
    solana: SolanaData,
}
#[derive(Deserialize)]
struct SolanaData {
    usd: f64,
}

#[derive(Clone)]
pub struct AppState {
    pub rpc_client: Arc<anchor_client::solana_client::rpc_client::RpcClient>,
    pub rpc_nonblocking_client: Arc<anchor_client::solana_client::nonblocking::rpc_client::RpcClient>,
    pub zeroslot_rpc_client: Arc<crate::services::zeroslot::ZeroSlotClient>,
    pub wallet: Arc<Keypair>,
    pub protocol_preference: SwapProtocol,
    pub jupiter_client: Arc<JupiterClient>,
}

#[derive(Clone, Debug)]
pub struct SwapConfig {
    pub swap_direction: SwapDirection,
    pub in_type: SwapInType,
    pub amount_in: f64,
    pub buy_slippage: u64,
    pub reverse: bool,
}

pub fn import_env_var(key: &str) -> String {
    match env::var(key){
        Ok(res) => res,
        Err(e) => {
            eprintln!("{}: {}", e, key);
            loop{}
        }
    }
}

// Zero slot health check URL
pub fn get_zero_slot_health_url() -> String {
    std::env::var("ZERO_SLOT_HEALTH").unwrap_or_else(|_| {
        // Log removed - using default value
        "https://ny1.0slot.trade/health".to_string()
    })
}

pub fn create_rpc_client() -> Result<Arc<anchor_client::solana_client::rpc_client::RpcClient>> {
    let rpc_http = import_env_var("RPC_HTTP");
    let timeout = Duration::from_secs(30); // 30 second timeout
    let rpc_client = anchor_client::solana_client::rpc_client::RpcClient::new_with_timeout_and_commitment(
        rpc_http,
        timeout,
        CommitmentConfig::processed(),
    );
    Ok(Arc::new(rpc_client))
}

pub async fn create_nonblocking_rpc_client(
) -> Result<Arc<anchor_client::solana_client::nonblocking::rpc_client::RpcClient>> {
    let rpc_http = import_env_var("RPC_HTTP");
    let timeout = Duration::from_secs(30); // 30 second timeout
    let rpc_client = anchor_client::solana_client::nonblocking::rpc_client::RpcClient::new_with_timeout_and_commitment(
        rpc_http,
        timeout,
        CommitmentConfig::processed(),
    );
    Ok(Arc::new(rpc_client))
}

pub async fn create_zeroslot_rpc_client() -> Result<Arc<crate::services::zeroslot::ZeroSlotClient>> {
    let client = crate::services::zeroslot::ZeroSlotClient::new(
        crate::services::zeroslot::ZERO_SLOT_URL.as_str()
    );
    Ok(Arc::new(client))
}


pub async fn create_coingecko_proxy() -> Result<f64, Error> {
 
    let url = "https://api.coingecko.com/api/v3/simple/price?ids=solana&vs_currencies=usd";

    let response = reqwest::get(url).await?;

    let body = response.json::<CoinGeckoResponse>().await?;
    // Get SOL price in USD
    let sol_price = body.solana.usd;
    Ok(sol_price)
}

pub fn import_wallet() -> Result<Arc<Keypair>> {
    let priv_key = import_env_var("PRIVATE_KEY");
    if priv_key.len() < 85 {
        eprintln!("Please check wallet priv key: Invalid length => {}", priv_key.len());
        loop{}
    }
    let wallet: Keypair = Keypair::from_base58_string(priv_key.as_str());

    Ok(Arc::new(wallet))
}