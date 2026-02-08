use std::sync::Arc;
use std::str::FromStr;
use anyhow::{anyhow, Result};
use colored::Colorize;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use anchor_client::solana_sdk::{
    signature::Keypair,
    pubkey::Pubkey,
    transaction::VersionedTransaction,
};
use anchor_client::solana_client::nonblocking::rpc_client::RpcClient;
use tokio::time::Duration;
use spl_associated_token_account::get_associated_token_address_with_program_id;

use crate::common::logger::Logger;

const JUPITER_API_URL: &str = "https://lite-api.jup.ag/swap/v1";
const JUPITER_SWAP_API_URL: &str = "https://lite-api.jup.ag/swap/v1";
const SOL_MINT: &str = "So11111111111111111111111111111111111111112";

#[derive(Debug, Serialize)]
struct QuoteRequest {
    #[serde(rename = "inputMint")]
    input_mint: String,
    #[serde(rename = "outputMint")]
    output_mint: String,
    amount: String,
    #[serde(rename = "slippageBps")]
    slippage_bps: u64,
}

#[derive(Debug, Deserialize, Serialize)] // Add Serialize derive
pub struct QuoteResponse {
    #[serde(rename = "inputMint")]
    pub input_mint: String,
    #[serde(rename = "inAmount")]
    pub in_amount: String,
    #[serde(rename = "outputMint")]
    pub output_mint: String,
    #[serde(rename = "outAmount")]
    pub out_amount: String,
    #[serde(rename = "otherAmountThreshold")]
    pub other_amount_threshold: String,
    #[serde(rename = "swapMode")]
    pub swap_mode: String,
    #[serde(rename = "slippageBps")]
    pub slippage_bps: u64,
    #[serde(rename = "platformFee")]
    pub platform_fee: Option<PlatformFee>,
    #[serde(rename = "priceImpactPct")]
    pub price_impact_pct: String,
    #[serde(rename = "routePlan")]
    pub route_plan: Vec<RoutePlanInfo>,
    #[serde(rename = "contextSlot")]
    pub context_slot: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlatformFee {
    pub amount: String,
    #[serde(rename = "feeBps")]
    pub fee_bps: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutePlanInfo {
    #[serde(rename = "swapInfo")]
    pub swap_info: SwapInfo,
    pub percent: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SwapInfo {
    pub label: String,
    #[serde(rename = "ammKey")]
    pub amm_key: String,
    #[serde(rename = "inputMint")]
    pub input_mint: String,
    #[serde(rename = "outputMint")]
    pub output_mint: String,
    #[serde(rename = "inAmount")]
    pub in_amount: String,
    #[serde(rename = "outAmount")]
    pub out_amount: String,
    // CRITICAL FIX: feeAmount and feeMint are optional fields according to Jupiter API
    // Some routes may not include fee information
    #[serde(rename = "feeAmount")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fee_amount: Option<String>,
    #[serde(rename = "feeMint")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fee_mint: Option<String>,
}

#[derive(Debug, Serialize)]
struct SwapRequest {
    #[serde(rename = "quoteResponse")]
    quote_response: QuoteResponse,
    #[serde(rename = "userPublicKey")]
    user_public_key: String,
    #[serde(rename = "wrapAndUnwrapSol")]
    wrap_and_unwrap_sol: bool,
    #[serde(rename = "dynamicComputeUnitLimit")]
    dynamic_compute_unit_limit: bool,
    #[serde(rename = "prioritizationFeeLamports")]
    prioritization_fee_lamports: PrioritizationFee,
}

#[derive(Debug, Serialize)]
struct PrioritizationFee {
    #[serde(rename = "priorityLevelWithMaxLamports")]
    priority_level_with_max_lamports: PriorityLevel,
}

#[derive(Debug, Serialize)]
struct PriorityLevel {
    #[serde(rename = "maxLamports")]
    max_lamports: u64,
    #[serde(rename = "priorityLevel")]
    priority_level: String,
}

#[derive(Debug, Deserialize)]
struct SwapResponse {
    #[serde(rename = "swapTransaction")]
    pub swap_transaction: String,
}

#[derive(Clone)]
pub struct JupiterClient {
    client: Client,
    rpc_client: Arc<RpcClient>,
    logger: Logger,
}

impl JupiterClient {
    pub fn new(rpc_client: Arc<RpcClient>) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .expect("Failed to create HTTP client");
            
        Self {
            client,
            rpc_client,
            logger: Logger::new("[JUPITER] => ".magenta().to_string()),
        }
    }

    /// Get a quote for swapping tokens
    pub async fn get_quote(
        &self,
        input_mint: &str,
        output_mint: &str,
        amount: u64,
        slippage_bps: u64,
    ) -> Result<QuoteResponse> {
        self.logger.log(format!("Getting Jupiter quote: {} -> {} (amount: {}, slippage: {}bps)", 
            input_mint, output_mint, amount, slippage_bps));

        let quote_request = QuoteRequest {
            input_mint: input_mint.to_string(),
            output_mint: output_mint.to_string(),
            amount: amount.to_string(),
            slippage_bps: 15000,  // fix to 15000 bps
        };

        let url = format!("{}/quote", JUPITER_API_URL);
        let response = self.client
            .get(&url)
            .query(&[
                ("inputMint", &quote_request.input_mint),
                ("outputMint", &quote_request.output_mint),
                ("amount", &quote_request.amount),
                ("slippageBps", &slippage_bps.to_string()), // Use the actual slippage parameter
            ])
            .send()
            .await?;

        if !response.status().is_success() {
            let error_text = response.text().await.unwrap_or_else(|_| "Unknown error".to_string());
            return Err(anyhow!("Jupiter quote API error: {}", error_text));
        }

        // Log the raw response for debugging
        let response_text = response.text().await?;
        self.logger.log(format!("Raw quote response: {}", &response_text[..std::cmp::min(500, response_text.len())]));
        
        let quote: QuoteResponse = serde_json::from_str(&response_text)
            .map_err(|e| anyhow!("Failed to parse quote response: {}. Response: {}", e, &response_text[..std::cmp::min(200, response_text.len())]))?;
        
        self.logger.log(format!("Jupiter quote received: {} {} -> {} {} (price impact: {}%)", 
            quote.in_amount, input_mint, quote.out_amount, output_mint, quote.price_impact_pct));

        Ok(quote)
    }

    /// Get swap transaction from Jupiter
    pub async fn get_swap_transaction(
        &self,
        quote: QuoteResponse,
        user_public_key: &Pubkey,
    ) -> Result<VersionedTransaction> {
        self.logger.log(format!("Getting Jupiter swap transaction for user: {}", user_public_key));

        let swap_request = SwapRequest {
            quote_response: quote,
            user_public_key: user_public_key.to_string(),
            wrap_and_unwrap_sol: true,
            dynamic_compute_unit_limit: true,
            prioritization_fee_lamports: PrioritizationFee {
                priority_level_with_max_lamports: PriorityLevel {
                    max_lamports: 1_000_000, // 0.001 SOL max priority fee
                    priority_level: "high".to_string(),
                },
            },
        };

        let url = format!("{}/swap", JUPITER_SWAP_API_URL);
        
        let response = self.client
            .post(&url)
            .json(&swap_request)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await.unwrap_or_else(|_| "Unknown error".to_string());
            self.logger.log(format!("Jupiter swap API error: Status {}, Response: {}", status, error_text).red().to_string());
            return Err(anyhow!("Swap API returned status: {} - {}", status, error_text));
        }

        let swap_response: SwapResponse = response.json().await?;
        
        // Decode the base64 transaction
        let transaction_bytes = base64::decode(&swap_response.swap_transaction)?;
        let transaction: VersionedTransaction = bincode::deserialize(&transaction_bytes)?;

        self.logger.log("Jupiter swap transaction received and decoded successfully".to_string());

        Ok(transaction)
    }

    /// Execute a token sell using Jupiter (complete flow)
    pub async fn sell_token_with_jupiter(
        &self,
        token_mint: &str,
        token_amount: u64,
        slippage_bps: u64,
        keypair: &Keypair,
    ) -> Result<String> {
        use tokio::time::{timeout, Duration};
        
        self.logger.log(format!("Starting Jupiter sell for token {} (amount: {}, slippage: {}bps)", 
            token_mint, token_amount, slippage_bps));

        // CRITICAL FIX: Add timeout to prevent hanging on slow RPC
        const RPC_TIMEOUT: Duration = Duration::from_secs(5);

        // All tokens are Token-2022, ensure ATA exists with Token-2022 program
        let mint_pubkey = Pubkey::from_str(token_mint)
            .map_err(|e| anyhow!("Invalid mint address: {}", e))?;
        
        self.logger.log(format!("✅ Ensuring Token-2022 ATA exists...").green().to_string());
        
        // Ensure ATA exists using Token-2022 program
        let token_2022_program = Pubkey::from_str("TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb")
            .map_err(|e| anyhow!("Failed to parse Token-2022 program ID: {}", e))?;
        
        let ata = get_associated_token_address_with_program_id(
            &keypair.pubkey(),
            &mint_pubkey,
            &token_2022_program,
        );
        
        // CRITICAL FIX: Add timeout to ATA check
        match timeout(RPC_TIMEOUT, self.rpc_client.get_account(&ata)).await {
            Ok(Ok(_)) => {
                self.logger.log(format!("✅ Token-2022 ATA already exists: {}", ata).green().to_string());
            }
            Ok(Err(_)) | Err(_) => {
                // Create ATA using Token-2022 program
                self.logger.log(format!("Creating Token-2022 ATA: {}", ata).yellow().to_string());
                
                use anchor_client::solana_sdk::transaction::Transaction;
                use spl_associated_token_account::instruction::create_associated_token_account_idempotent;
                
                let create_ata_ix = create_associated_token_account_idempotent(
                    &keypair.pubkey(),
                    &keypair.pubkey(),
                    &mint_pubkey,
                    &token_2022_program,
                );
                
                // CRITICAL FIX: Add timeout to get_latest_blockhash
                let recent_blockhash = match timeout(RPC_TIMEOUT, self.rpc_client.get_latest_blockhash()).await {
                    Ok(Ok(bh)) => bh,
                    Ok(Err(e)) => return Err(anyhow!("Failed to get blockhash for ATA creation: {}", e)),
                    Err(_) => return Err(anyhow!("Blockhash request timed out for ATA creation")),
                };
                
                let mut tx = Transaction::new_with_payer(
                    &[create_ata_ix],
                    Some(&keypair.pubkey()),
                );
                tx.sign(&[keypair], recent_blockhash);
                
                // CRITICAL FIX: Use send_transaction (non-blocking) with timeout instead of send_and_confirm_transaction
                // This prevents the bot from getting stuck if ATA creation hangs
                let send_result = timeout(
                    Duration::from_secs(2),
                    self.rpc_client.send_transaction(&tx)
                ).await;
                
                match send_result {
                    Ok(Ok(sig)) => {
                        self.logger.log(format!("✅ Token-2022 ATA creation sent: {}", sig).green().to_string());
                    }
                    Ok(Err(e)) => {
                        // If ATA creation fails, it might already exist (idempotent), continue anyway
                        self.logger.log(format!("⚠️ ATA creation returned error (may already exist): {}", e).yellow().to_string());
                    }
                    Err(_timeout) => {
                        self.logger.log(format!("⚠️ ATA creation timed out (may already exist), continuing...").yellow().to_string());
                    }
                }
            }
        }

        // Get quote
        self.logger.log("Getting Jupiter quote...".to_string());
        let quote = self.get_quote(
            token_mint,
            SOL_MINT,
            token_amount,
            slippage_bps,
        ).await?;

        self.logger.log(format!("Quote received, getting swap transaction..."));
        
        // Get swap transaction
        let mut transaction = self.get_swap_transaction(quote, &keypair.pubkey()).await?;

        // CRITICAL FIX: Add timeout to get_latest_blockhash - this is a common bottleneck
        self.logger.log("Getting recent blockhash...".to_string());
        let recent_blockhash = match timeout(RPC_TIMEOUT, self.rpc_client.get_latest_blockhash()).await {
            Ok(Ok(bh)) => bh,
            Ok(Err(e)) => return Err(anyhow!("Failed to get recent blockhash: {}", e)),
            Err(_) => return Err(anyhow!("Blockhash request timed out after {}s", RPC_TIMEOUT.as_secs())),
        };
        transaction.message.set_recent_blockhash(recent_blockhash);

        // For VersionedTransaction, we need to manually create the signature
        use anchor_client::solana_sdk::signer::Signer;
        let message_data = transaction.message.serialize();
        let signature = keypair.sign_message(&message_data);
        
        // Find the position of the keypair in the account keys to place the signature
        let account_keys = transaction.message.static_account_keys();
        if let Some(signer_index) = account_keys.iter().position(|key| *key == keypair.pubkey()) {
            // Ensure we have enough signatures
            if transaction.signatures.len() <= signer_index {
                transaction.signatures.resize(signer_index + 1, anchor_client::solana_sdk::signature::Signature::default());
            }
            transaction.signatures[signer_index] = signature;
        } else {
            return Err(anyhow!("Keypair not found in transaction account keys"));
        }

        // CRITICAL FIX: Add timeout to send_transaction - this is the final bottleneck
        self.logger.log("Sending transaction to network...".to_string());
        let signature = match timeout(RPC_TIMEOUT, self.rpc_client.send_transaction(&transaction)).await {
            Ok(Ok(sig)) => sig,
            Ok(Err(e)) => return Err(anyhow!("Failed to send transaction: {}", e)),
            Err(_) => return Err(anyhow!("Transaction send timed out after {}s", RPC_TIMEOUT.as_secs())),
        };

        self.logger.log(format!("Jupiter sell transaction sent: {}", signature).green().to_string());

        Ok(signature.to_string())
    }
} 