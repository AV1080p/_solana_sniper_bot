use std::sync::Arc;
use std::str::FromStr;
use anyhow::{Result, anyhow};
use colored::Colorize;
use anchor_client::solana_sdk::{
    instruction::Instruction,
    signature::Keypair,
    system_instruction,
    transaction::Transaction,
};
use std::env;
use anchor_client::solana_sdk::pubkey::Pubkey;
use spl_token::ui_amount_to_amount;
use solana_sdk::signature::Signer;
use tokio::time::Instant;
use once_cell::sync::Lazy;
use reqwest::Client;
use crate::{
    common::{
        logger::Logger,
        config::TransactionLandingMode,
    },
    services::{
        zeroslot,
    },
};

// prioritization fee = UNIT_PRICE * UNIT_LIMIT
fn get_unit_price() -> u64 {
    env::var("UNIT_PRICE")
        .ok()
        .and_then(|v| u64::from_str(&v).ok())
        .unwrap_or(20000)
}

fn get_unit_limit() -> u32 {
    env::var("UNIT_LIMIT")
        .ok()
        .and_then(|v| u32::from_str(&v).ok())
        .unwrap_or(200_000)
}

// Create a static HTTP client with optimized configuration for FlashBlock API
static HTTP_CLIENT: Lazy<Client> = Lazy::new(|| {
   let client = reqwest::Client::new();
   client
});

pub async fn new_signed_and_send_zeroslot(
    zeroslot_rpc_client: Arc<crate::services::zeroslot::ZeroSlotClient>,
    recent_blockhash: solana_sdk::hash::Hash,
    keypair: &Keypair,
    mut instructions: Vec<Instruction>,
    logger: &Logger,
    is_buy: bool,
    slot: Option<u64>,
) -> Result<Vec<String>> {
    let tip_account = zeroslot::get_tip_account()?;
    let start_time = Instant::now();
    let mut txs: Vec<String> = vec![];
    
    // zeroslot tip, the upper limit is 0.1
    let tip = zeroslot::get_tip_value().await?;
    let tip_lamports = ui_amount_to_amount(tip, spl_token::native_mint::DECIMALS);

    let zeroslot_tip_instruction = 
        system_instruction::transfer(&keypair.pubkey(), &tip_account, tip_lamports);
        
        let unit_limit = get_unit_limit(); // TODO: update in mev boost
        let unit_price = get_unit_price(); // TODO: update in mev boost
        let modify_compute_units =
        solana_sdk::compute_budget::ComputeBudgetInstruction::set_compute_unit_limit(unit_limit);
        let add_priority_fee =
        solana_sdk::compute_budget::ComputeBudgetInstruction::set_compute_unit_price(unit_price);
        //  temporarily disabled for reducing gas fee during testing
        instructions.insert(1, modify_compute_units);
        instructions.insert(2, add_priority_fee);
        
        instructions.push(zeroslot_tip_instruction); // zeroslot is different with others.

    // If this is a BUY, add Lighthouse sysvar slot assertion as the last instruction
    if is_buy {
        if let Some(slot_value) = slot {
            const LIGHTHOUSE_PROGRAM_ID: &str = "L2TExMFKdjpN9kozasaurPirfHy9P8sbXoAN1qA3S95";
            let lighthouse_program_id = Pubkey::from_str(LIGHTHOUSE_PROGRAM_ID)?;

            let mut lighthouse_data = Vec::new();
            // Instruction discriminator for AssertSysvarClock
            lighthouse_data.extend_from_slice(&[15]);
            // Log level (1 byte): 0 = Silent
            lighthouse_data.push(1u8);
            // Assertion type (1 byte): 0 = Slot assertion
            lighthouse_data.push(0u8);
            // Slot value (8 bytes, little endian)
            println!("slot_value: {}", slot_value);
            let slot_u64 = slot_value;
            lighthouse_data.extend_from_slice(&slot_u64.to_le_bytes());
            // Operator (1 byte): 5 = <= (as per reference)
            lighthouse_data.push(5u8);

            let lighthouse_ix = Instruction {
                program_id: lighthouse_program_id,
                accounts: vec![],
                data: lighthouse_data,
            };
           //  sysvar assertion is very important, but I igored it for now for testing temperarily, after complete testing, I will add it back
        //    instructions.push(lighthouse_ix);
        }
    }
   
    let nonce_account_pubkey = env::var("NONCE_ACCOUNT")
        .ok()
        .and_then(|v| Pubkey::from_str(&v).ok())
        .unwrap_or(Pubkey::default());

    // send init tx
    let nonce_instruction = system_instruction::advance_nonce_account(
        &nonce_account_pubkey,
        &keypair.pubkey(),
    );

    instructions.insert(0, nonce_instruction);

    let txn = Transaction::new_signed_with_payer(
        &instructions,
        Some(&keypair.pubkey()),
        &vec![keypair],
        recent_blockhash,
    );

    let tx_result = zeroslot_rpc_client.send_transaction(&txn).await;
    
    match tx_result {
        Ok(signature) => {
            txs.push(signature.to_string());
            logger.log(
                format!("[TXN-ELAPSED(ZEROSLOT)]: {:?}", start_time.elapsed())
                    .yellow()
                    .to_string(),
            );
        }
        Err(_) => {
            // Convert the error to a Send-compatible form
            return Err(anyhow::anyhow!("zeroslot send_transaction failed"));
        }
    };

    Ok(txs)
}
/// Send transaction using normal RPC without any service or tips
pub async fn new_signed_and_send_normal(
    rpc_client: Arc<anchor_client::solana_client::nonblocking::rpc_client::RpcClient>,
    recent_blockhash: anchor_client::solana_sdk::hash::Hash,
    keypair: &Keypair,
    mut instructions: Vec<Instruction>,
    logger: &Logger,
) -> Result<Vec<String>> {
    let start_time = Instant::now();
    
    // Add compute budget instructions for priority fee
    // let unit_limit = 200000;
    // let unit_price = 20000;
    // let modify_compute_units =
    //     solana_sdk::compute_budget::ComputeBudgetInstruction::set_compute_unit_limit(unit_limit);
    // let add_priority_fee =
    //     solana_sdk::compute_budget::ComputeBudgetInstruction::set_compute_unit_price(unit_price);
    // instructions.insert(0, modify_compute_units);
    // instructions.insert(1, add_priority_fee);
    
    // Create and send transaction
    let txn = Transaction::new_signed_with_payer(
        &instructions,
        Some(&keypair.pubkey()),
        &vec![keypair],
        recent_blockhash,
    );

    match rpc_client.send_transaction(&txn).await {
        Ok(signature) => {
            logger.log(
                format!("[TXN-ELAPSED(NORMAL)]: {:?}", start_time.elapsed())
                    .yellow()
                    .to_string(),
            );
            Ok(vec![signature.to_string()])
        }
        Err(e) => Err(anyhow!("Failed to send normal transaction: {}", e))
    }
}

/// Universal transaction landing function that routes to the appropriate service
pub async fn new_signed_and_send_with_landing_mode(
    transaction_landing_mode: TransactionLandingMode,
    app_state: &crate::common::config::AppState,
    recent_blockhash: anchor_client::solana_sdk::hash::Hash,
    keypair: &Keypair,
    instructions: Vec<Instruction>,
    logger: &Logger,
    is_buy: bool,
    slot: Option<u64>,
) -> Result<Vec<String>> {
    // Route to the appropriate service
    match transaction_landing_mode {
        TransactionLandingMode::Zeroslot => {
            logger.log("Using Zeroslot for transaction landing".green().to_string());
            new_signed_and_send_zeroslot(
                app_state.zeroslot_rpc_client.clone(),
                recent_blockhash,
                keypair,
                instructions,
                logger,
                is_buy,
                slot,
            ).await
        },
        TransactionLandingMode::Normal => {
            logger.log("Using Normal RPC for transaction landing".green().to_string());
            new_signed_and_send_normal(
                app_state.rpc_nonblocking_client.clone(),
                recent_blockhash,
                keypair,
                instructions,
                logger,
            ).await
        },
    }
}

