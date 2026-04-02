use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use backend::config::AppConfig;
use backend::db::Database;
use backend::onchain::{BlockchainClient, OnchainWallet};
use backend::wallet::{WalletConfig, WalletHandle, open_wallet};
use cdk::Amount;
use cdk::amount::SplitTarget;
use cdk::nuts::nut00::KnownMethod;
use cdk::nuts::{MintQuoteState, PaymentMethod};
use cdk::wallet::{ReceiveOptions, SendOptions};
use clap::{Parser, Subcommand};
use tokio::time::sleep;

const DEFAULT_ELECTRUM_RETRY: u8 = 5;
const DEFAULT_ELECTRUM_TIMEOUT_SECS: u8 = 30;

#[derive(Parser)]
#[command(name = "shuestand-wallet", about = "Manage Cashu and on-chain wallets")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    #[command(name = "cashuwallet")]
    CashuWallet(CashuWalletArgs),
    #[command(name = "onchainwallet")]
    OnchainWallet(OnchainWalletArgs),
}

#[derive(Parser)]
struct CashuWalletArgs {
    /// Override the mint URL (defaults to CASHU_MINT_URL)
    #[arg(long)]
    mint: Option<String>,
    /// Override the wallet directory (defaults to CASHU_WALLET_DIR or ~/.shuestand/cashu)
    #[arg(long)]
    wallet_dir: Option<PathBuf>,
    #[command(subcommand)]
    command: CashuCommand,
}

#[derive(Subcommand)]
enum CashuCommand {
    /// Show wallet balances (spendable, pending, reserved)
    Balance,
    /// Request an invoice from the mint and wait for payment to credit the wallet
    Invoice {
        /// Amount to mint (sats)
        amount: u64,
        /// Use Bolt12 invoices instead of Bolt11
        #[arg(long)]
        bolt12: bool,
    },
    /// Export tokens out of the wallet (reduces float)
    Send {
        /// Amount to export (sats)
        amount: u64,
        /// Optional file to write the token into (stdout if omitted)
        #[arg(long)]
        output: Option<PathBuf>,
    },
    /// Import an encoded token into the wallet (adds to float)
    Receive {
        /// Raw token (cashuB)
        #[arg(long)]
        token: Option<String>,
        /// File that contains the token (defaults to /tmp/cashu_token.txt)
        #[arg(long)]
        file: Option<PathBuf>,
    },
    /// Pay a Lightning invoice by melting proofs (Bolt11 by default)
    Pay {
        /// BOLT11 (default) or Bolt12 offer string
        invoice: String,
        /// Treat the invoice string as Bolt12
        #[arg(long)]
        bolt12: bool,
    },
}

#[derive(Parser)]
struct OnchainWalletArgs {
    /// Override DATABASE_URL for wallet storage (defaults to env)
    #[arg(long)]
    database_url: Option<String>,
    #[command(subcommand)]
    command: OnchainCommand,
}

#[derive(Subcommand)]
enum OnchainCommand {
    /// Derive a fresh deposit address from the hot wallet
    Deposit,
    /// Show the on-chain wallet balance (syncs first)
    Balance,
    /// Send sats to an on-chain address from the hot wallet
    Withdraw {
        /// Destination address (must match configured network)
        address: String,
        /// Amount to send (sats)
        amount: u64,
        /// Fee rate in sats/vbyte (defaults to WITHDRAWAL_PAYOUT_FEE_RATE_VB)
        #[arg(long)]
        fee_rate: Option<f32>,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    match &cli.command {
        Command::CashuWallet(args) => handle_cashu_wallet(args).await?,
        Command::OnchainWallet(args) => handle_onchain_wallet(args).await?,
    }

    Ok(())
}

async fn handle_cashu_wallet(args: &CashuWalletArgs) -> Result<()> {
    let config = resolve_cashu_config(args)?;
    let wallet = open_wallet(&config)
        .await
        .with_context(|| format!("opening wallet at {}", config.base_dir().display()))?;

    match &args.command {
        CashuCommand::Balance => show_balance(&wallet).await?,
        CashuCommand::Invoice { amount, bolt12 } => {
            let method = if *bolt12 {
                KnownMethod::Bolt12
            } else {
                KnownMethod::Bolt11
            };
            request_invoice(&wallet, *amount, method).await?;
        }
        CashuCommand::Send { amount, output } => {
            send_tokens(&wallet, *amount, output.as_ref()).await?;
        }
        CashuCommand::Receive { token, file } => {
            receive_token(&wallet, token.as_deref(), file.as_ref()).await?;
        }
        CashuCommand::Pay { invoice, bolt12 } => {
            pay_invoice(&wallet, invoice, *bolt12).await?;
        }
    }

    Ok(())
}

fn resolve_cashu_config(args: &CashuWalletArgs) -> Result<WalletConfig> {
    let mint = args
        .mint
        .clone()
        .or_else(|| std::env::var("CASHU_MINT_URL").ok())
        .ok_or_else(|| anyhow!("--mint or CASHU_MINT_URL must be set"))?;
    let wallet_dir = args
        .wallet_dir
        .clone()
        .or_else(|| std::env::var("CASHU_WALLET_DIR").ok().map(PathBuf::from));

    Ok(WalletConfig::new(mint, wallet_dir))
}

async fn handle_onchain_wallet(args: &OnchainWalletArgs) -> Result<()> {
    let (wallet, config) = init_onchain_wallet(args).await?;
    match &args.command {
        OnchainCommand::Deposit => {
            let address = wallet.next_external_address().await?;
            println!("Next deposit address: {address}");
        }
        OnchainCommand::Balance => {
            wallet.sync().await?;
            let balance = wallet.balance().await?;
            println!("On-chain wallet balance:");
            println!("  Confirmed        : {} sats", balance.confirmed);
            println!("  Trusted pending  : {} sats", balance.trusted_pending);
            println!("  Untrusted pending: {} sats", balance.untrusted_pending);
            println!("  Immature         : {} sats", balance.immature);
        }
        OnchainCommand::Withdraw {
            address,
            amount,
            fee_rate,
        } => {
            if *amount == 0 {
                anyhow::bail!("amount must be greater than zero");
            }
            wallet.sync().await?;
            let selected_fee = fee_rate
                .clone()
                .unwrap_or(config.withdrawal_payout_fee_rate_vb);
            if selected_fee <= 0.0 {
                anyhow::bail!("fee rate must be greater than zero");
            }
            let txid = wallet
                .send_to_address(address, *amount, selected_fee)
                .await?;
            println!(
                "Broadcasted withdrawal of {} sats at {} sat/vB\nTxid: {txid}",
                amount, selected_fee
            );
        }
    }

    Ok(())
}

async fn init_onchain_wallet(args: &OnchainWalletArgs) -> Result<(OnchainWallet, AppConfig)> {
    let config = AppConfig::from_env();
    let database_url = args
        .database_url
        .clone()
        .or_else(|| std::env::var("DATABASE_URL").ok())
        .ok_or_else(|| anyhow!("DATABASE_URL must be set for onchainwallet commands"))?;
    let db = Database::connect(&database_url)
        .await
        .with_context(|| format!("connecting to {database_url}"))?;
    let spend_descriptor = config
        .bitcoin_spend_descriptor
        .clone()
        .ok_or_else(|| anyhow!("BITCOIN_SPEND_DESCRIPTOR or BITCOIN_WALLET_SEED must be set"))?;
    let blockchain = build_blockchain_client(&config)?;
    let wallet = OnchainWallet::new(
        &db,
        spend_descriptor.as_str(),
        config.bitcoin_change_descriptor.as_deref(),
        config.bitcoin_network,
        blockchain,
    )
    .await?;
    Ok((wallet, config))
}

fn build_blockchain_client(config: &AppConfig) -> Result<BlockchainClient> {
    if let Some(url) = config.bitcoin_electrum_url.as_deref() {
        Ok(BlockchainClient::from_electrum_config(
            url,
            config.bitcoin_electrum_socks5.as_deref(),
            DEFAULT_ELECTRUM_RETRY,
            Some(DEFAULT_ELECTRUM_TIMEOUT_SECS),
            config.bitcoin_electrum_validate_domain,
        )?)
    } else if let Some(base) = config.esplora_base_url.as_deref() {
        Ok(BlockchainClient::from_esplora(base))
    } else {
        anyhow::bail!(
            "BITCOIN_ELECTRUM_URL or BITCOIN_ESPLORA_BASE_URL must be set for onchainwallet commands"
        );
    }
}

async fn show_balance(wallet: &WalletHandle) -> Result<()> {
    let guard = wallet.lock().await;
    let spendable = guard.total_balance().await?.to_u64();
    let pending = guard.total_pending_balance().await?.to_u64();
    let reserved = guard.total_reserved_balance().await?.to_u64();
    println!("Wallet balances:");
    println!("  Spendable: {spendable} sats");
    println!("  Pending  : {pending} sats");
    println!("  Reserved : {reserved} sats");
    Ok(())
}

async fn request_invoice(wallet: &WalletHandle, amount: u64, method: KnownMethod) -> Result<()> {
    if amount == 0 {
        anyhow::bail!("amount must be greater than zero");
    }

    let quote_id = {
        let guard = wallet.lock().await;
        let quote = guard
            .mint_quote(
                cdk::nuts::PaymentMethod::Known(method),
                Some(Amount::from(amount)),
                None,
                None,
            )
            .await?;
        println!(
            "Pay this invoice to credit {amount} sats:\n{}",
            quote.request
        );
        quote.id
    };

    loop {
        sleep(Duration::from_secs(5)).await;
        let state = {
            let guard = wallet.lock().await;
            guard.check_mint_quote_status(&quote_id).await?
        };
        match state.state {
            MintQuoteState::Paid | MintQuoteState::Issued => break,
            other => {
                println!("Waiting for payment... state={other:?}");
            }
        }
    }

    {
        let guard = wallet.lock().await;
        guard.mint(&quote_id, SplitTarget::default(), None).await?;
    }

    println!("Minted {amount} sats into the wallet");
    Ok(())
}

async fn send_tokens(wallet: &WalletHandle, amount: u64, output: Option<&PathBuf>) -> Result<()> {
    if amount == 0 {
        anyhow::bail!("amount must be greater than zero");
    }

    let token_string = {
        let guard = wallet.lock().await;
        let prepared = guard
            .prepare_send(Amount::from(amount), SendOptions::default())
            .await?;
        prepared.confirm(None).await?.to_string()
    };

    if let Some(path) = output {
        fs::write(path, &token_string)
            .with_context(|| format!("writing token to {}", path.display()))?;
        println!(
            "Exported {amount} sats into {} (keep this token safe)",
            path.display()
        );
    } else {
        println!("Exported token (keep this secret):\n{token_string}");
    }

    Ok(())
}

async fn receive_token(
    wallet: &WalletHandle,
    token_arg: Option<&str>,
    file_arg: Option<&PathBuf>,
) -> Result<()> {
    let token = if let Some(raw) = token_arg {
        raw.to_string()
    } else {
        let path = file_arg
            .cloned()
            .unwrap_or_else(|| PathBuf::from("/tmp/cashu_token.txt"));
        fs::read_to_string(&path)
            .with_context(|| format!("reading token file {}", path.display()))?
    };

    let token = token.trim();
    if token.is_empty() {
        anyhow::bail!("token input was empty");
    }

    let amount = wallet
        .lock()
        .await
        .receive(token, ReceiveOptions::default())
        .await?
        .to_u64();
    println!("Received {amount} sats into the wallet");
    Ok(())
}

async fn pay_invoice(wallet: &WalletHandle, invoice: &str, bolt12: bool) -> Result<()> {
    if invoice.trim().is_empty() {
        anyhow::bail!("invoice must not be empty");
    }

    let method = if bolt12 {
        KnownMethod::Bolt12
    } else {
        KnownMethod::Bolt11
    };

    let finalized = {
        let guard = wallet.lock().await;
        let quote = guard
            .melt_quote(PaymentMethod::Known(method), invoice, None, None)
            .await?;
        let prepared = guard.prepare_melt(&quote.id, HashMap::new()).await?;
        prepared.confirm().await?
    };

    println!(
        "Paid {} sats (fees {}), state {:?}",
        finalized.amount(),
        finalized.fee_paid(),
        finalized.state()
    );
    if let Some(preimage) = finalized.payment_proof() {
        println!("Payment preimage: {preimage}");
    }
    Ok(())
}
