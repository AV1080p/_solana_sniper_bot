use anchor_client::solana_sdk::{pubkey::Pubkey, signature::Keypair, signer::Signer, instruction::Instruction, rent::Rent, system_instruction};
use solana_program_pack::Pack;
use spl_token_2022::{
    extension::StateWithExtensionsOwned,
    state::{Account, Mint},
};
use spl_token_client::{
    client::{ProgramClient, ProgramRpcClient, ProgramRpcClientSendTransaction},
    token::{Token, TokenError, TokenResult},
};
use std::sync::Arc;
use anyhow::{Result, anyhow};
use spl_associated_token_account::instruction::create_associated_token_account_idempotent;


pub fn get_token_address(
    client: Arc<anchor_client::solana_client::nonblocking::rpc_client::RpcClient>,
    keypair: Arc<Keypair>,
    address: &Pubkey,
    owner: &Pubkey,
) -> Pubkey {
    let token_client = Token::new(
        Arc::new(ProgramRpcClient::new(
            client.clone(),
            ProgramRpcClientSendTransaction,
        )),
        &spl_token::ID,
        address,
        None,
        Arc::new(Keypair::from_bytes(&keypair.to_bytes()).expect("failed to copy keypair")),
    );
    token_client.get_associated_token_address(owner)
}

pub async fn get_account_info(
    client: Arc<anchor_client::solana_client::nonblocking::rpc_client::RpcClient>,
    address: Pubkey,
    account: Pubkey,
) -> TokenResult<StateWithExtensionsOwned<Account>> {
    // Fetch from RPC directly (no cache needed - bot uses create_associated_token_account_idempotent)
    let program_client = Arc::new(ProgramRpcClient::new(
        client.clone(),
        ProgramRpcClientSendTransaction,
    ));
    let account_data = program_client
        .get_account(account)
        .await
        .map_err(TokenError::Client)?
        .ok_or(TokenError::AccountNotFound)
        .inspect_err(|_err| {
            // logger.log(format!(
            //     "get_account_info: {} {}: mint {}",
            //     account, err, address
            // ));
        })?;

    if account_data.owner != spl_token::ID {
        return Err(TokenError::AccountInvalidOwner);
    }
    let account_info = StateWithExtensionsOwned::<Account>::unpack(account_data.data)?;
    if account_info.base.mint != address {
        return Err(TokenError::AccountInvalidMint);
    }

    Ok(account_info)
}

pub async fn get_mint_info(
    client: Arc<anchor_client::solana_client::nonblocking::rpc_client::RpcClient>,
    _keypair: Arc<Keypair>,
    address: Pubkey,
) -> TokenResult<StateWithExtensionsOwned<Mint>> {
    // Fetch from RPC (cache removed)
    let program_client = Arc::new(ProgramRpcClient::new(
        client.clone(),
        ProgramRpcClientSendTransaction,
    ));
    let account = program_client
        .get_account(address)
        .await
        .map_err(TokenError::Client)?
        .ok_or(TokenError::AccountNotFound)
        .inspect_err(|err| println!("{} {}: mint {}", address, err, address))?;

    if account.owner != spl_token::ID {
        return Err(TokenError::AccountInvalidOwner);
    }

    let mint_result = StateWithExtensionsOwned::<Mint>::unpack(account.data).map_err(Into::into);
    let decimals: Option<u8> = None;
    if let (Ok(mint), Some(decimals)) = (&mint_result, decimals) {
        if decimals != mint.base.decimals {
            return Err(TokenError::InvalidDecimals);
        }
    }

    mint_result
}

/// Check if a token account exists
pub async fn account_exists(
    rpc_client: Arc<anchor_client::solana_client::nonblocking::rpc_client::RpcClient>,
    account: &Pubkey,
) -> Result<bool, anyhow::Error> {
    // Just check if the account exists without validating the mint
    // (no cache needed - bot uses create_associated_token_account_idempotent)
    match rpc_client.get_account_with_commitment(account, rpc_client.commitment()).await {
        Ok(response) => {
            match response.value {
                Some(acc) => {
                    // Check if the account is owned by the token program
                    if acc.owner == spl_token::ID {
                        Ok(true)
                    } else {
                        Ok(false)
                    }
                },
                None => Ok(false),
            }
        },
        Err(e) => Err(anyhow!("Error checking account: {}, account: {}", e, account)),
    }
}

/// Check if a specific token account exists and validates the mint
pub async fn verify_token_account(
    rpc_client: Arc<anchor_client::solana_client::nonblocking::rpc_client::RpcClient>,
    mint: &Pubkey,
    account: &Pubkey,
) -> Result<bool, anyhow::Error> {
    // Fetch directly (no cache needed - bot uses create_associated_token_account_idempotent)
    match get_account_info(rpc_client, *mint, *account).await {
        Ok(_) => Ok(true),
        Err(TokenError::AccountNotFound) => Ok(false),
        Err(TokenError::AccountInvalidMint) => Ok(false),
        Err(TokenError::AccountInvalidOwner) => Ok(false),
        Err(e) => Err(anyhow!("Error checking account: {} , account: {}", e, account)),
    }
}

/// Get multiple token accounts in a single RPC call
pub async fn get_multiple_token_accounts(
    rpc_client: Arc<anchor_client::solana_client::nonblocking::rpc_client::RpcClient>,
    accounts: &[Pubkey],
) -> Result<Vec<Option<StateWithExtensionsOwned<Account>>>, anyhow::Error> {
    let mut result = Vec::with_capacity(accounts.len());
    
    // Fetch all accounts directly (no cache needed - bot uses create_associated_token_account_idempotent)
    let fetched_accounts = rpc_client.get_multiple_accounts(accounts).await?;
    
    for maybe_account in fetched_accounts.iter() {
        if let Some(account_data) = maybe_account {
            if account_data.owner == spl_token::ID {
                if let Ok(token_account) = StateWithExtensionsOwned::<Account>::unpack(account_data.data.clone()) {
                    result.push(Some(token_account));
                } else {
                    result.push(None);
                }
            } else {
                result.push(None);
            }
        } else {
            result.push(None);
        }
    }
    
    Ok(result)
}

/// Create a wrapped SOL account with a specific amount
pub fn create_wsol_account_with_amount(
    owner: Pubkey,
    amount: u64,
) -> Result<(Pubkey, Vec<Instruction>), anyhow::Error> {
    let wsol_account = Keypair::new();
    let wsol_account_pubkey = wsol_account.pubkey();
    
    let instructions = vec![
        // Create account
        system_instruction::create_account(
            &owner,
            &wsol_account_pubkey,
            amount + Rent::default().minimum_balance(Account::LEN),
            Account::LEN as u64,
            &spl_token::id(),
        ),
        // Initialize as token account
        spl_token::instruction::initialize_account(
            &spl_token::id(),
            &wsol_account_pubkey,
            &spl_token::native_mint::id(),
            &owner,
        )?,
    ];
    
    Ok((wsol_account_pubkey, instructions))
}

/// Create a wrapped SOL account (without funding)
pub fn create_wsol_account(
    owner: Pubkey,
) -> Result<(Pubkey, Vec<Instruction>), anyhow::Error> {
    let mut instructions = Vec::new();
    
    // Create the associated token account for WSOL
    instructions.push(
        create_associated_token_account_idempotent(
            &owner,
            &owner,
            &spl_token::native_mint::id(),
            &spl_token::ID,
        )
    );
    
    // Get the WSOL ATA address using the SPL token function directly
    let wsol_account = spl_associated_token_account::get_associated_token_address(
        &owner,
        &spl_token::native_mint::id()
    );
    
    Ok((wsol_account, instructions))
}

/// Close a token account
pub fn close_account(
    _owner: Pubkey,
    token_account: Pubkey,
    destination: Pubkey,
    authority: Pubkey,
    signers: &[&Pubkey],
) -> Result<Instruction, anyhow::Error> {
    Ok(spl_token::instruction::close_account(
        &spl_token::id(),
        &token_account,
        &destination,
        &authority,
        signers,
    )?)
}
