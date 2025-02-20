use futures::StreamExt;
use solana_client::nonblocking::pubsub_client::PubsubClient;
use solana_client::rpc_config::{RpcTransactionLogsConfig, RpcTransactionLogsFilter};
use solana_sdk::pubkey::Pubkey;
use std::str::FromStr;
use base64::{engine::general_purpose::STANDARD as base64, Engine};
use bs58;
use byteorder::{LittleEndian, ReadBytesExt};
use std::io::{Cursor, Read};
use solana_sdk::{
    instruction::{AccountMeta, Instruction},
    signer::Signer,
    transaction::Transaction,
    system_program,
    signer::keypair::{Keypair, read_keypair_file},
    commitment_config::CommitmentConfig,
};
use solana_client::rpc_client::RpcClient;
use spl_associated_token_account::instruction::create_associated_token_account_idempotent;
use spl_token::instruction::close_account;
use std::collections::HashMap;
use std::time::{SystemTime, Duration};
use tokio::sync::mpsc;
use std::sync::Arc;
use parking_lot::RwLock;
use std::error::Error;
use std::fmt;
use std::string::FromUtf8Error;
use std::num::ParseIntError;
use hex::FromHexError;
use base64::DecodeError;
use std::array::TryFromSliceError;
use solana_sdk::pubkey::ParsePubkeyError;
use solana_program::program_error::ProgramError;
use tokio::time::{interval};

#[derive(Debug, Copy, Clone)]
#[repr(C)]
pub struct TradeEvent {
    pub mint: Pubkey,
    pub sol_amount: u64,
    pub token_amount: u64,
    pub is_buy: bool,
    pub user: Pubkey,
    pub timestamp: u64,
    pub virtual_sol_reserves: u64,
    pub virtual_token_reserves: u64,
}

#[derive(Debug, Copy, Clone)]
#[repr(C)]
pub struct BondingLayout {
    pub virtual_token_reserves: u64,
    pub virtual_sol_reserves: u64,
    pub real_token_reserves: u64,
    pub real_sol_reserves: u64,
    pub supply: u64,
    pub completed: u8,
}

#[derive(Debug, Copy, Clone)]
#[repr(C)]
pub struct SplMintLayout {
    pub mint_authority_option: u32,
    pub mint_authority: Pubkey,
    pub supply: u64,
    pub decimals: u8,
    pub is_initialized: u8,
    pub freeze_authority_option: u32,
    pub freeze_authority: Pubkey,
}

#[derive(Debug)]
struct PumpKeys {
    metadata: Pubkey,
    user_associated_token: Pubkey,
    associated_bonding_curve: Pubkey,
    user: Pubkey,
    mint: Pubkey,
    bonding: Pubkey,
    mint_authority: Pubkey,
    global: Pubkey,
    mpl_token_metadata: Pubkey,
    system_program: Pubkey,
    token_program: Pubkey,
    associated_token_program: Pubkey,
    rent: Pubkey,
    event_authority: Pubkey,
    program: Pubkey,
    sell_event_authority: Pubkey,
    fee_recipient: Pubkey,
}

#[derive(Debug)]
struct ProcessError(String);

impl fmt::Display for ProcessError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl Error for ProcessError {}
impl From<FromUtf8Error> for ProcessError {
    fn from(e: FromUtf8Error) -> Self {
        ProcessError(e.to_string())
    }
}

impl From<ParseIntError> for ProcessError {
    fn from(e: ParseIntError) -> Self {
        ProcessError(e.to_string())
    }
}

impl From<FromHexError> for ProcessError {
    fn from(e: FromHexError) -> Self {
        ProcessError(e.to_string())
    }
}

impl From<DecodeError> for ProcessError {
    fn from(e: DecodeError) -> Self {
        ProcessError(e.to_string())
    }
}

impl From<TryFromSliceError> for ProcessError {
    fn from(e: TryFromSliceError) -> Self {
        ProcessError(e.to_string())
    }
}

impl From<std::io::Error> for ProcessError {
    fn from(e: std::io::Error) -> Self {
        ProcessError(e.to_string())
    }
}

impl From<solana_client::client_error::ClientError> for ProcessError {
    fn from(e: solana_client::client_error::ClientError) -> Self {
        ProcessError(e.to_string())
    }
}

impl From<Box<dyn Error>> for ProcessError {
    fn from(e: Box<dyn Error>) -> Self {
        ProcessError(e.to_string())
    }
}

impl From<&str> for ProcessError {
    fn from(e: &str) -> Self {
        ProcessError(e.to_string())
    }
}

impl From<ParsePubkeyError> for ProcessError {
    fn from(e: ParsePubkeyError) -> Self {
        ProcessError(e.to_string())
    }
}

impl From<spl_token::error::TokenError> for ProcessError {
    fn from(e: spl_token::error::TokenError) -> Self {
        ProcessError(e.to_string())
    }
}

impl From<ProgramError> for ProcessError {
    fn from(e: ProgramError) -> Self {
        ProcessError(e.to_string())
    }
}

impl From<String> for ProcessError {
    fn from(e: String) -> Self {
        ProcessError(e)
    }
}

async fn get_owner_ata(mint: &Pubkey, pubkey: &Pubkey) -> Result<Pubkey, Box<dyn std::error::Error>> {
    let token_program_id = spl_token::id();

    let seeds = &[
        pubkey.as_ref(),
        token_program_id.as_ref(),
        mint.as_ref(),
    ];

    let (ata, _) = Pubkey::find_program_address(
        seeds,
        &spl_associated_token_account::id(),
    );
    Ok(ata)
}

async fn get_pump_keys(mint: &Pubkey, wallet: &Keypair) -> Result<PumpKeys, ProcessError> {
    let program = Pubkey::from_str("6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P")?;
    let mpl = Pubkey::from_str("metaqbxxUerdq28cj1RbAWkYQm3ybzjb6a8bt518x1s")?;
    let mint_authority = Pubkey::from_str("TSLvdd1pWpHVjahSpsvCXUbgwsL3JAcvokwaKt1eokM")?;
    let event_authority = Pubkey::from_str("Ce6TQqeHC9p8KetsN6JsjHK7UTZk7nasjjnr7XxXp9F1")?;
    let fee_recipient = Pubkey::from_str("CebN5WGQ4jvEPvsVU4EoHEpgzq1VV7AbicfhtW4xC9iM")?;
    let (global, _) = Pubkey::find_program_address(&[b"global"], &program);
    let (metadata, _) = Pubkey::find_program_address(
        &[b"metadata", mpl.as_ref(), mint.as_ref()],
        &mpl,
    );
    let user_associated_token = get_owner_ata(mint, &wallet.pubkey()).await?;
    let (bonding, _) = Pubkey::find_program_address(
        &[b"bonding-curve", mint.as_ref()],
        &program,
    );
    let associated_bonding_curve = get_owner_ata(mint, &bonding).await?;

    Ok(PumpKeys {
        metadata,
        user_associated_token,
        associated_bonding_curve,
        user: wallet.pubkey(),
        mint: *mint,
        bonding,
        mint_authority,
        global,
        mpl_token_metadata: mpl,
        system_program: system_program::id(),
        token_program: spl_token::id(),
        associated_token_program: spl_associated_token_account::id(),
        rent: solana_sdk::sysvar::rent::id(),
        event_authority,
        program,
        sell_event_authority: event_authority,
        fee_recipient,
    })
}

async fn buy_pump_token(
    client: &RpcClient,
    wallet: &Keypair,
    pump_keys: &PumpKeys,
    buy_tokens_amount_raw: u64,
) -> Result<String, ProcessError> {
    let max_sol_cost_raw = 99999999999u64;
    let create_ata = create_associated_token_account_idempotent(
        &wallet.pubkey(),
        &wallet.pubkey(),
        &pump_keys.mint,
        &pump_keys.token_program,
    );
    let mut buffer = vec![0u8; 24];
    buffer[0..8].copy_from_slice(&hex::decode("66063d1201daebea")?);
    buffer[8..16].copy_from_slice(&buy_tokens_amount_raw.to_le_bytes());
    buffer[16..24].copy_from_slice(&max_sol_cost_raw.to_le_bytes());
    let account_metas = vec![
        AccountMeta::new_readonly(pump_keys.global, false),
        AccountMeta::new(pump_keys.fee_recipient, false),
        AccountMeta::new_readonly(pump_keys.mint, false),
        AccountMeta::new(pump_keys.bonding, false),
        AccountMeta::new(pump_keys.associated_bonding_curve, false),
        AccountMeta::new(pump_keys.user_associated_token, false),
        AccountMeta::new(wallet.pubkey(), true),
        AccountMeta::new_readonly(pump_keys.system_program, false),
        AccountMeta::new_readonly(pump_keys.token_program, false),
        AccountMeta::new_readonly(pump_keys.rent, false),
        AccountMeta::new_readonly(pump_keys.sell_event_authority, false),
        AccountMeta::new_readonly(pump_keys.program, false),
    ];
    let instruction = Instruction::new_with_bytes(
        pump_keys.program,
        &buffer,
        account_metas,
    );
    let mut transaction = Transaction::new_with_payer(
        &[create_ata, instruction],
        Some(&wallet.pubkey()),
    );

    let recent_blockhash = client.get_latest_blockhash()?;
    transaction.sign(&[wallet], recent_blockhash);

    let signature = client.send_transaction(&transaction)?;
    Ok(signature.to_string())
}
async fn sell_pump_token(
    client: &RpcClient,
    wallet: &Keypair,
    pump_keys: &PumpKeys,
    sell_tokens_amount_raw: u64,
) -> Result<String, ProcessError> {
    let close_ata = close_account(
        &pump_keys.token_program,
        &pump_keys.user_associated_token,
        &wallet.pubkey(),
        &wallet.pubkey(),
        &[],
    )?;
    let mut buffer = vec![0u8; 24];
    buffer[0..8].copy_from_slice(&[51, 230, 133, 164, 1, 127, 131, 173]);
    buffer[8..16].copy_from_slice(&sell_tokens_amount_raw.to_le_bytes());
    buffer[16..24].copy_from_slice(&0u64.to_le_bytes()); // minSolReceived
    let account_metas = vec![
        AccountMeta::new_readonly(pump_keys.global, false),
        AccountMeta::new(pump_keys.fee_recipient, false),
        AccountMeta::new_readonly(pump_keys.mint, false),
        AccountMeta::new(pump_keys.bonding, false),
        AccountMeta::new(pump_keys.associated_bonding_curve, false),
        AccountMeta::new(pump_keys.user_associated_token, false),
        AccountMeta::new(wallet.pubkey(), true),
        AccountMeta::new_readonly(pump_keys.system_program, false),
        AccountMeta::new_readonly(pump_keys.associated_token_program, false),
        AccountMeta::new_readonly(pump_keys.token_program, false),
        AccountMeta::new_readonly(pump_keys.sell_event_authority, false),
        AccountMeta::new_readonly(pump_keys.program, false),
    ];
    let sell_instruction = Instruction::new_with_bytes(
        pump_keys.program,
        &buffer,
        account_metas,
    );
    let mut transaction = Transaction::new_with_payer(
        &[sell_instruction, close_ata],
        Some(&wallet.pubkey()),
    );

    let recent_blockhash = client.get_latest_blockhash()?;
    transaction.sign(&[wallet], recent_blockhash);

    let signature = client.send_transaction(&transaction)?;
    Ok(signature.to_string())
}
#[derive(Debug, Clone)]
struct TokenInfo {
    token_name: String,
    entry_price: f64,
    current_price: f64,
    is_bought: bool,
    is_sold: bool,
    timestamp: SystemTime,
}
const MAX_CONCURRENT_TRADES: usize = 3;
const ABANDON_TIMEOUT: Duration = Duration::from_secs(8);
const BUY_AMOUNT: u64 = 30000000000;
const SELL_AMOUNT: u64 = 30000000000;
const PROFIT_THRESHOLD: f64 = 0.04;

async fn process_log(
    log: String,
    client: &Arc<RpcClient>,
    wallet: &Arc<Keypair>,
    active_trades: &Arc<RwLock<HashMap<Pubkey, TokenInfo>>>
) -> Result<(), ProcessError> {
    match process_program_data(&log) {
        Ok(event) => {
            match event {
                ProgramEvent::Creation { mint, name } => {
                    if active_trades.read().len() < MAX_CONCURRENT_TRADES {
                        let pump_keys = get_pump_keys(&mint, wallet).await?;
                        match buy_pump_token(client, wallet, &pump_keys, BUY_AMOUNT).await {
                            Ok(signature) => {
                                println!("buy: {} | mint: {} | amount: {} | tx: {}",
                                    name, mint, BUY_AMOUNT, signature);
                                active_trades.write().insert(mint, TokenInfo {
                                    token_name: name,
                                    entry_price: 0.0,
                                    current_price: 0.0,
                                    is_bought: true,
                                    is_sold: false,
                                    timestamp: SystemTime::now(),
                                });
                            }
                            Err(e) => eprintln!("error buying {}: {}", name, e),
                        }
                    }

                    check_and_sell_abandoned_trades(client, wallet, active_trades).await?;
                }
                ProgramEvent::Trade { mint, price } => {
                    let should_sell = {
                        let mut trades = active_trades.write();
                        if let Some(token_info) = trades.get_mut(&mint) {
                            if token_info.is_bought && !token_info.is_sold {
                                token_info.current_price = price;

                                if token_info.entry_price == 0.0 {
                                    token_info.entry_price = price;
                                    None
                                } else {
                                    let price_change = (price - token_info.entry_price) / token_info.entry_price;
                                    println!("update: {} | change: {:.2}% | current: {} SOL | entry: {} SOL | mint: {}",
                                        token_info.token_name,
                                        price_change * 100.0,
                                        price,
                                        token_info.entry_price,
                                        mint);

                                    if price_change >= PROFIT_THRESHOLD {
                                        Some(SELL_AMOUNT)
                                    } else {
                                        None
                                    }
                                }
                            } else {
                                None
                            }
                        } else {
                            None
                        }
                    };

                    if let Some(amount) = should_sell {
                        let token_info = active_trades.read().get(&mint).cloned();
                        if let Some(info) = token_info {
                            let pump_keys = get_pump_keys(&mint, wallet).await?;
                            match sell_pump_token(client, wallet, &pump_keys, amount).await {
                                Ok(signature) => {
                                    println!("sold: {} | current: {} SOL | entry: {} SOL | mint: {} | tx: {}",
                                        info.token_name,
                                        info.current_price,
                                        info.entry_price,
                                        mint,
                                        signature);
                                    active_trades.write().remove(&mint);
                                }
                                Err(e) => eprintln!("error selling {}: {}", info.token_name, e),
                            }
                        }
                    }
                }
                ProgramEvent::Unknown => {}
            }
        }
        Err(e) => eprintln!("error processing log: {}", e),
    }
    Ok(())
}
async fn check_and_sell_abandoned_trades(
    client: &Arc<RpcClient>,
    wallet: &Arc<Keypair>,
    active_trades: &Arc<RwLock<HashMap<Pubkey, TokenInfo>>>
) -> Result<(), ProcessError> {
    let abandoned: Vec<_> = {
        let trades = active_trades.read();
        trades.iter()
            .filter(|(_, info)| {
                info.is_bought &&
                !info.is_sold &&
                SystemTime::now().duration_since(info.timestamp).unwrap_or(Duration::ZERO) > ABANDON_TIMEOUT
            })
            .map(|(k, info)| (*k, info.token_name.clone()))
            .collect()
    };

    for (mint, name) in abandoned {
        println!("dropping {} ({}): exceeded {} second timeout", name, mint, ABANDON_TIMEOUT.as_secs());
        sell_abandoned_token(client, wallet, &mint, active_trades).await?;
    }

    Ok(())
}
async fn sell_abandoned_token(
    client: &Arc<RpcClient>,
    wallet: &Arc<Keypair>,
    mint: &Pubkey,
    active_trades: &Arc<RwLock<HashMap<Pubkey, TokenInfo>>>
) -> Result<(), ProcessError> {
    let token_info = {
        let trades = active_trades.read();
        trades.get(mint).cloned()
    };
    if let Some(info) = token_info {
        if info.is_bought && !info.is_sold {
            let pump_keys = get_pump_keys(mint, wallet).await?;
            match sell_pump_token(client, wallet, &pump_keys, SELL_AMOUNT).await {
                Ok(signature) => {
                    println!("dropped: {} | mint: {} | tx: {}",
                        info.token_name,
                        mint,
                        signature);
                    active_trades.write().remove(mint);
                }
                Err(e) => eprintln!("error abandoning {}: {}", info.token_name, e),
            }
        }
    }

    Ok(())
}
enum ProgramEvent {
    Creation { mint: Pubkey, name: String },
    Trade { mint: Pubkey, price: f64 },
    Unknown,
}
fn process_program_data(log: &str) -> Result<ProgramEvent, ProcessError> {
    let data_base64 = log.replace("Program data: ", "");
    let bytes = base64.decode(data_base64)?;

    let disc = hex::encode(&bytes[0..8]);
    if disc == "1b72a94ddeeb6376" {
        let mut cursor = Cursor::new(&bytes[8..]);
        let name_len = cursor.read_u32::<LittleEndian>()?;
        let name_end = 12 + name_len as usize;
        let name = String::from_utf8(bytes[12..name_end].to_vec())?;
        let symbol_len = (&bytes[name_end..name_end + 4]).read_u32::<LittleEndian>()?;
        let symbol_end = name_end + 4 + symbol_len as usize;
        let symbol = String::from_utf8(bytes[name_end + 4..symbol_end].to_vec())?;
        let uri_len = (&bytes[symbol_end..symbol_end + 4]).read_u32::<LittleEndian>()?;
        let uri_end = symbol_end + 4 + uri_len as usize;
        let uri = String::from_utf8(bytes[symbol_end + 4..uri_end].to_vec())?;
        let owner = bs58::encode(&bytes[bytes.len() - 32..]).into_string();
        let bonding = bs58::encode(&bytes[bytes.len() - 64..bytes.len() - 32]).into_string();
        let mint = bs58::encode(&bytes[bytes.len() - 96..bytes.len() - 64]).into_string();
        Ok(ProgramEvent::Creation {
            mint: Pubkey::from_str(&mint)?,
            name,
        })
    } else if disc == "bddb7fd34ee661ee" {
        let mut cursor = Cursor::new(&bytes[8..]);
        let mut mint_bytes = [0u8; 32];
        cursor.read_exact(&mut mint_bytes)?;
        let mint = Pubkey::try_from(&mint_bytes[..]).unwrap();
        let sol_amount = cursor.read_u64::<LittleEndian>()?;
        let token_amount = cursor.read_u64::<LittleEndian>()?;
        let is_buy = cursor.read_u8()? != 0;
        let mut user_bytes = [0u8; 32];
        cursor.read_exact(&mut user_bytes)?;
        let user = Pubkey::try_from(&user_bytes[..]).unwrap();
        let timestamp = cursor.read_u64::<LittleEndian>()?;
        let virtual_sol_reserves = cursor.read_u64::<LittleEndian>()?;
        let virtual_token_reserves = cursor.read_u64::<LittleEndian>()?;
        let price = if virtual_token_reserves > 0 {
            virtual_sol_reserves as f64 / virtual_token_reserves as f64
        } else {
            0.0
        };
        Ok(ProgramEvent::Trade {
            mint,
            price,
        })
	} else {
		Ok(ProgramEvent::Unknown)
    }
}

pub const PUMP_PROGRAM_ID: &str = "6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P";
pub const MPL_PROGRAM_ID: &str = "metaqbxxUerdq28cj1RbAWkYQm3ybzjb6a8bt518x1s";

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    let rpc_client = Arc::new(RpcClient::new_with_commitment(
        "your rpc url here",
        CommitmentConfig::processed()
    ));
    let ws_url = "wss://your rpc url here";
    let pubsub_client = PubsubClient::new(ws_url).await?;

    let wallet = Arc::new(read_keypair_file("./kp.json").map_err(|e| ProcessError(e.to_string()))?);
    let active_trades = Arc::new(RwLock::new(HashMap::new()));
    let (tx, mut rx) = mpsc::channel(100_000);
    let program_id = Pubkey::from_str("6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P")?;
    let (mut logs_subscription, _unsubscribe) = pubsub_client
        .logs_subscribe(
            RpcTransactionLogsFilter::Mentions(vec![program_id.to_string()]),
            RpcTransactionLogsConfig {
                commitment: Some(CommitmentConfig::processed()),
            },
        )
        .await?;
    println!("🚀 bot started! monitoring for new tokens...");
    println!("✨ max concurrent trades: {}", MAX_CONCURRENT_TRADES);
    println!("💰 buy amount: {} tokens", BUY_AMOUNT);
    println!("📈 profit target: {}%", PROFIT_THRESHOLD * 100.0);
    println!("⏱️ drop timeout: {} seconds", ABANDON_TIMEOUT.as_secs());
    let process_client = Arc::clone(&rpc_client);
    let process_wallet = Arc::clone(&wallet);
    let process_trades = Arc::clone(&active_trades);
    tokio::task::spawn(async move {
        while let Some(log_msg) = rx.recv().await {
            if let Err(e) = process_log(log_msg, &process_client, &process_wallet, &process_trades).await {
                eprintln!("error processing log: {}", e);
            }
        }
    });
    let balance_client = Arc::clone(&rpc_client);
    let balance_wallet = Arc::clone(&wallet);
    tokio::spawn(async move {
        let mut interval = interval(Duration::from_secs(30));
        loop {
            interval.tick().await;
            match balance_client.get_balance(&balance_wallet.pubkey()) {
                Ok(balance) => {
                    println!("balance: {} SOL", balance as f64 / 1_000_000_000.0);
                }
                Err(e) => eprintln!("error fetching balance: {}", e),
            }
        }
    });
    while let Some(logs) = logs_subscription.next().await {
        for log in logs.value.logs {
            if log.contains("Program data:") {
                if log.contains("1b72a94ddeeb6376") {
                    if let Err(e) = tx.try_send(log) {
                        eprintln!("error sending creation log: {}", e);
                    }
                } else {
                    if let Err(e) = tx.send(log).await {
                        eprintln!("error sending log: {}", e);
                    }
                }
            }
        }
    }

    Ok(())
}
