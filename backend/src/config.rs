use std::path::PathBuf;
use std::str::FromStr;
use std::time::Duration;

use anyhow::{Context, anyhow};
use bdk::bitcoin::Network;
use bdk::bitcoin::bip32::{DerivationPath, ExtendedPrivKey, ExtendedPubKey};
use bdk::bitcoin::secp256k1::Secp256k1;
use bdk::keys::bip39::{Language, Mnemonic};

const DEFAULT_ADDRESS_POOL_TARGET: u32 = 20;
const DEFAULT_DEPOSIT_TARGET_CONFIRMATIONS: u8 = 3;
const DEFAULT_DEPOSIT_MIN_SATS: u64 = 50_000;
const DEFAULT_WITHDRAWAL_TARGET_CONFIRMATIONS: u8 = 1;
const CONFIRMATION_POLL_INTERVAL_SECS: u64 = 30;
const DEFAULT_WITHDRAWAL_WORKER_INTERVAL_SECS: u64 = 15;
const DEFAULT_WITHDRAWAL_MAX_ATTEMPTS: u32 = 5;
const DEFAULT_WITHDRAWAL_PAYOUT_FEE_RATE_VB: f32 = 2.0;
const DEFAULT_DEPOSIT_WORKER_INTERVAL_SECS: u64 = 10;
const DEFAULT_DEPOSIT_WORKER_MAX_ATTEMPTS: u32 = 5;

const DEFAULT_FLOAT_TARGET_SATS: u64 = 500_000;
const DEFAULT_FLOAT_MIN_RATIO: f32 = 0.5;
const DEFAULT_FLOAT_MAX_RATIO: f32 = 2.0;
const DEFAULT_FLOAT_GUARD_INTERVAL_SECS: u64 = 30;
const DEFAULT_WITHDRAWAL_MIN_SATS: u64 = 50_000;
const DEFAULT_WITHDRAWAL_FEE_BUFFER_SATS: u64 = 500;
const DEFAULT_FLOAT_DRIFT_ALERT_RATIO: f32 = 0.1;
const DEFAULT_SINGLE_REQUEST_RATIO: f64 = 0.5;
const DEFAULT_PENDING_DEPOSIT_TTL_SECS: u64 = 600;
const DEFAULT_MAX_PENDING_DEPOSITS_PER_SESSION: u64 = 2;

#[derive(Clone)]
pub struct AppConfig {
    pub bitcoin_descriptor: Option<String>,
    pub bitcoin_spend_descriptor: Option<String>,
    pub bitcoin_change_descriptor: Option<String>,
    pub bitcoin_network: Network,
    pub esplora_base_url: Option<String>,
    pub bitcoin_electrum_url: Option<String>,
    pub bitcoin_electrum_socks5: Option<String>,
    pub bitcoin_electrum_validate_domain: bool,
    pub address_pool_target: u32,
    pub deposit_target_confirmations: u8,
    pub withdrawal_target_confirmations: u8,
    pub confirmation_poll_interval: Duration,
    pub withdrawal_worker_interval: Duration,
    pub withdrawal_worker_enabled: bool,
    pub withdrawal_worker_max_attempts: u32,
    pub withdrawal_payout_fee_rate_vb: f32,
    pub deposit_worker_interval: Duration,
    pub deposit_worker_enabled: bool,
    pub deposit_worker_max_attempts: u32,
    pub cashu_mint_url: Option<String>,
    pub public_base_url: Option<String>,
    pub cashu_wallet_dir: Option<PathBuf>,
    pub wallet_api_token: Option<String>,
    pub float_target_sats: u64,
    pub float_min_ratio: f32,
    pub float_max_ratio: f32,
    pub float_guard_interval: Duration,
    pub deposit_min_sats: u64,
    pub withdrawal_min_sats: u64,
    pub withdrawal_fee_buffer_sats: u64,
    pub float_drift_alert_ratio: f32,
    pub single_request_cap_ratio: f64,
    pub pending_deposit_ttl_secs: u64,
    pub max_pending_deposits_per_session: u64,
    pub float_alert_webhook_url: Option<String>,
    pub transaction_webhook_url: Option<String>,
    pub security_alert_webhook_url: Option<String>,
    pub trust_proxy_headers: bool,
    pub cors_allowed_origins: Vec<String>,
    pub fee_estimator: FeeEstimatorSettings,
}

impl AppConfig {
    pub fn from_env() -> Self {
        let mut bitcoin_descriptor = std::env::var("BITCOIN_DESCRIPTOR").ok();
        let mut bitcoin_spend_descriptor = std::env::var("BITCOIN_SPEND_DESCRIPTOR").ok();
        let mut bitcoin_change_descriptor = std::env::var("BITCOIN_CHANGE_DESCRIPTOR").ok();
        let bitcoin_wallet_seed = std::env::var("BITCOIN_WALLET_SEED").ok();
        let bitcoin_wallet_passphrase = std::env::var("BITCOIN_WALLET_PASSPHRASE").ok();
        let bitcoin_network = std::env::var("BITCOIN_NETWORK")
            .ok()
            .and_then(|v| match v.trim().to_lowercase().as_str() {
                "bitcoin" | "mainnet" => Some(Network::Bitcoin),
                "testnet" => Some(Network::Testnet),
                "signet" => Some(Network::Signet),
                "regtest" => Some(Network::Regtest),
                _ => None,
            })
            .unwrap_or(Network::Regtest);
        let esplora_base_url = std::env::var("BITCOIN_ESPLORA_BASE_URL").ok();
        let bitcoin_electrum_url = std::env::var("BITCOIN_ELECTRUM_URL").ok();
        let bitcoin_electrum_socks5 = std::env::var("BITCOIN_ELECTRUM_SOCKS5").ok();
        let bitcoin_electrum_validate_domain = std::env::var("BITCOIN_ELECTRUM_VALIDATE_DOMAIN")
            .map(|v| {
                !matches!(
                    v.trim().to_ascii_lowercase().as_str(),
                    "0" | "false" | "off"
                )
            })
            .unwrap_or(true);
        let address_pool_target = std::env::var("ADDRESS_POOL_TARGET")
            .ok()
            .and_then(|v| v.parse::<u32>().ok())
            .filter(|v| *v > 0)
            .unwrap_or(DEFAULT_ADDRESS_POOL_TARGET);
        let deposit_target_confirmations = std::env::var("DEPOSIT_TARGET_CONFIRMATIONS")
            .ok()
            .and_then(|v| v.parse::<u8>().ok())
            .map(|v| v.max(1))
            .unwrap_or(DEFAULT_DEPOSIT_TARGET_CONFIRMATIONS);
        let withdrawal_target_confirmations = std::env::var("WITHDRAWAL_TARGET_CONFIRMATIONS")
            .ok()
            .and_then(|v| v.parse::<u8>().ok())
            .filter(|v| *v > 0)
            .unwrap_or(DEFAULT_WITHDRAWAL_TARGET_CONFIRMATIONS);
        let confirmation_poll_interval = Duration::from_secs(
            std::env::var("CONFIRMATION_POLL_INTERVAL_SECS")
                .ok()
                .and_then(|v| v.parse::<u64>().ok())
                .filter(|v| *v > 0)
                .unwrap_or(CONFIRMATION_POLL_INTERVAL_SECS),
        );
        let withdrawal_worker_interval = Duration::from_secs(
            std::env::var("WITHDRAWAL_WORKER_INTERVAL_SECS")
                .ok()
                .and_then(|v| v.parse::<u64>().ok())
                .filter(|v| *v > 0)
                .unwrap_or(DEFAULT_WITHDRAWAL_WORKER_INTERVAL_SECS),
        );
        let withdrawal_worker_enabled = std::env::var("WITHDRAWAL_WORKER_ENABLED")
            .map(|v| {
                !matches!(
                    v.trim().to_ascii_lowercase().as_str(),
                    "0" | "false" | "off"
                )
            })
            .unwrap_or(true);
        let withdrawal_worker_max_attempts = std::env::var("WITHDRAWAL_WORKER_MAX_ATTEMPTS")
            .ok()
            .and_then(|v| v.parse::<u32>().ok())
            .filter(|v| *v > 0)
            .unwrap_or(DEFAULT_WITHDRAWAL_MAX_ATTEMPTS);
        let withdrawal_payout_fee_rate_vb = std::env::var("WITHDRAWAL_PAYOUT_FEE_RATE_VB")
            .ok()
            .and_then(|v| v.parse::<f32>().ok())
            .filter(|v| *v > 0.0)
            .unwrap_or(DEFAULT_WITHDRAWAL_PAYOUT_FEE_RATE_VB);

        let fee_estimator_refresh_interval = Duration::from_secs(
            std::env::var("FEE_ESTIMATOR_REFRESH_SECS")
                .ok()
                .and_then(|v| v.parse::<u64>().ok())
                .filter(|v| *v > 0)
                .unwrap_or(60),
        );
        let fee_estimator_fast_blocks = std::env::var("FEE_ESTIMATOR_FAST_BLOCKS")
            .ok()
            .and_then(|v| v.parse::<u32>().ok())
            .filter(|v| *v > 0)
            .unwrap_or(1);
        let fee_estimator_economy_blocks = std::env::var("FEE_ESTIMATOR_ECONOMY_BLOCKS")
            .ok()
            .and_then(|v| v.parse::<u32>().ok())
            .filter(|v| *v > 0)
            .unwrap_or(3);
        let fee_estimator_min_sat_per_vb = std::env::var("FEE_ESTIMATOR_MIN_SAT_PER_VB")
            .ok()
            .and_then(|v| v.parse::<f32>().ok())
            .filter(|v| *v > 0.0)
            .unwrap_or(1.0);
        let fee_estimator_max_sat_per_vb = std::env::var("FEE_ESTIMATOR_MAX_SAT_PER_VB")
            .ok()
            .and_then(|v| v.parse::<f32>().ok())
            .filter(|v| *v > fee_estimator_min_sat_per_vb)
            .unwrap_or(500.0);

        let fee_estimator = FeeEstimatorSettings {
            rpc_url: std::env::var("BITCOIND_RPC_URL").ok(),
            rpc_user: std::env::var("BITCOIND_RPC_USER").ok(),
            rpc_password: std::env::var("BITCOIND_RPC_PASSWORD").ok(),
            refresh_interval: fee_estimator_refresh_interval,
            fast_target_blocks: fee_estimator_fast_blocks,
            economy_target_blocks: fee_estimator_economy_blocks,
            min_sat_per_vb: fee_estimator_min_sat_per_vb,
            max_sat_per_vb: fee_estimator_max_sat_per_vb,
            default_fast_sat_per_vb: withdrawal_payout_fee_rate_vb,
            default_economy_sat_per_vb: withdrawal_payout_fee_rate_vb,
        };
        let withdrawal_min_sats = std::env::var("WITHDRAWAL_MIN_SATS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .filter(|v| *v > 0)
            .unwrap_or(DEFAULT_WITHDRAWAL_MIN_SATS);
        let withdrawal_fee_buffer_sats = std::env::var("WITHDRAWAL_FEE_BUFFER_SATS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(DEFAULT_WITHDRAWAL_FEE_BUFFER_SATS);
        let deposit_min_sats = std::env::var("DEPOSIT_MIN_SATS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .filter(|v| *v > 0)
            .unwrap_or(DEFAULT_DEPOSIT_MIN_SATS);

        let deposit_worker_interval = Duration::from_secs(
            std::env::var("DEPOSIT_WORKER_INTERVAL_SECS")
                .ok()
                .and_then(|v| v.parse::<u64>().ok())
                .filter(|v| *v > 0)
                .unwrap_or(DEFAULT_DEPOSIT_WORKER_INTERVAL_SECS),
        );
        let deposit_worker_enabled = std::env::var("DEPOSIT_WORKER_ENABLED")
            .map(|v| {
                !matches!(
                    v.trim().to_ascii_lowercase().as_str(),
                    "0" | "false" | "off"
                )
            })
            .unwrap_or(true);
        let deposit_worker_max_attempts = std::env::var("DEPOSIT_WORKER_MAX_ATTEMPTS")
            .ok()
            .and_then(|v| v.parse::<u32>().ok())
            .filter(|v| *v > 0)
            .unwrap_or(DEFAULT_DEPOSIT_WORKER_MAX_ATTEMPTS);

        let cashu_mint_url = std::env::var("CASHU_MINT_URL").ok();
        let public_base_url = std::env::var("PUBLIC_BASE_URL")
            .ok()
            .map(|value| value.trim().trim_end_matches('/').to_string())
            .filter(|value| !value.is_empty());
        let cashu_wallet_dir = std::env::var("CASHU_WALLET_DIR").ok().map(PathBuf::from);
        let wallet_api_token = std::env::var("WALLET_API_TOKEN").ok();

        let float_target_sats = std::env::var("FLOAT_TARGET_SATS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .filter(|v| *v > 0)
            .unwrap_or(DEFAULT_FLOAT_TARGET_SATS);
        let float_min_ratio = std::env::var("FLOAT_MIN_RATIO")
            .ok()
            .and_then(|v| v.parse::<f32>().ok())
            .filter(|v| *v > 0.0)
            .unwrap_or(DEFAULT_FLOAT_MIN_RATIO);
        let float_max_ratio = std::env::var("FLOAT_MAX_RATIO")
            .ok()
            .and_then(|v| v.parse::<f32>().ok())
            .filter(|v| *v > float_min_ratio)
            .unwrap_or(DEFAULT_FLOAT_MAX_RATIO);
        let float_guard_interval = Duration::from_secs(
            std::env::var("FLOAT_GUARD_INTERVAL_SECS")
                .ok()
                .and_then(|v| v.parse::<u64>().ok())
                .filter(|v| *v > 0)
                .unwrap_or(DEFAULT_FLOAT_GUARD_INTERVAL_SECS),
        );
        let float_drift_alert_ratio = std::env::var("FLOAT_DRIFT_ALERT_RATIO")
            .ok()
            .and_then(|v| v.parse::<f32>().ok())
            .filter(|v| *v > 0.0)
            .unwrap_or(DEFAULT_FLOAT_DRIFT_ALERT_RATIO);
        let single_request_cap_ratio = std::env::var("SINGLE_REQUEST_CAP_RATIO")
            .ok()
            .and_then(|v| v.parse::<f64>().ok())
            .filter(|v| (0.0..=1.0).contains(v))
            .unwrap_or(DEFAULT_SINGLE_REQUEST_RATIO);
        let pending_deposit_ttl_secs = std::env::var("PENDING_DEPOSIT_TTL_SECS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .filter(|v| *v > 0)
            .unwrap_or(DEFAULT_PENDING_DEPOSIT_TTL_SECS);
        let max_pending_deposits_per_session =
            std::env::var("MAX_PENDING_DEPOSITS_PER_SESSION")
                .ok()
                .and_then(|v| v.parse::<u64>().ok())
                .filter(|v| *v > 0)
                .unwrap_or(DEFAULT_MAX_PENDING_DEPOSITS_PER_SESSION);
        let float_alert_webhook_url = std::env::var("FLOAT_ALERT_WEBHOOK_URL").ok();
        let transaction_webhook_url = std::env::var("TRANSACTION_WEBHOOK_URL").ok();
        let security_alert_webhook_url = std::env::var("SECURITY_ALERT_WEBHOOK_URL").ok();
        let trust_proxy_headers = std::env::var("TRUST_PROXY_HEADERS")
            .map(|v| {
                matches!(
                    v.trim().to_ascii_lowercase().as_str(),
                    "1" | "true" | "yes" | "on"
                )
            })
            .unwrap_or(false);
        let cors_allowed_origins = std::env::var("CORS_ALLOWED_ORIGINS")
            .ok()
            .map(|raw| {
                raw.split(',')
                    .map(str::trim)
                    .filter(|v| !v.is_empty())
                    .map(|v| v.to_string())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        if bitcoin_wallet_seed.is_some()
            && (bitcoin_descriptor.is_none()
                || bitcoin_spend_descriptor.is_none()
                || bitcoin_change_descriptor.is_none())
        {
            let derived = derive_descriptors_from_seed(
                bitcoin_wallet_seed.as_ref().unwrap(),
                bitcoin_wallet_passphrase.as_deref(),
                bitcoin_network,
            )
            .unwrap_or_else(|err| {
                panic!("failed to derive descriptors from BITCOIN_WALLET_SEED: {err}")
            });
            if bitcoin_descriptor.is_none() {
                bitcoin_descriptor = Some(derived.public_descriptor);
            }
            if bitcoin_spend_descriptor.is_none() {
                bitcoin_spend_descriptor = Some(derived.spend_descriptor);
            }
            if bitcoin_change_descriptor.is_none() {
                bitcoin_change_descriptor = Some(derived.change_descriptor);
            }
        }

        Self {
            bitcoin_descriptor,
            bitcoin_spend_descriptor,
            bitcoin_change_descriptor,
            bitcoin_network,
            esplora_base_url,
            bitcoin_electrum_url,
            bitcoin_electrum_socks5,
            bitcoin_electrum_validate_domain,
            address_pool_target,
            deposit_target_confirmations,
            withdrawal_target_confirmations,
            confirmation_poll_interval,
            withdrawal_worker_interval,
            withdrawal_worker_enabled,
            withdrawal_worker_max_attempts,
            withdrawal_payout_fee_rate_vb,
            deposit_worker_interval,
            deposit_worker_enabled,
            deposit_worker_max_attempts,
            cashu_mint_url,
            public_base_url,
            cashu_wallet_dir,
            wallet_api_token,
            float_target_sats,
            float_min_ratio,
            float_max_ratio,
            float_guard_interval,
            deposit_min_sats,
            withdrawal_min_sats,
            withdrawal_fee_buffer_sats,
            float_drift_alert_ratio,
            single_request_cap_ratio,
            pending_deposit_ttl_secs,
            max_pending_deposits_per_session,
            float_alert_webhook_url,
            transaction_webhook_url,
            security_alert_webhook_url,
            trust_proxy_headers,
            cors_allowed_origins,
            fee_estimator,
        }
    }
}

#[derive(Clone)]
pub struct FeeEstimatorSettings {
    pub rpc_url: Option<String>,
    pub rpc_user: Option<String>,
    pub rpc_password: Option<String>,
    pub refresh_interval: Duration,
    pub fast_target_blocks: u32,
    pub economy_target_blocks: u32,
    pub min_sat_per_vb: f32,
    pub max_sat_per_vb: f32,
    pub default_fast_sat_per_vb: f32,
    pub default_economy_sat_per_vb: f32,
}

struct SeedDerivedDescriptors {
    public_descriptor: String,
    spend_descriptor: String,
    change_descriptor: String,
}

fn derive_descriptors_from_seed(
    seed_phrase: &str,
    passphrase: Option<&str>,
    network: Network,
) -> Result<SeedDerivedDescriptors, anyhow::Error> {
    let mnemonic = Mnemonic::parse_in(Language::English, seed_phrase)
        .context("invalid BITCOIN_WALLET_SEED mnemonic")?;
    let seed = mnemonic.to_seed(passphrase.unwrap_or(""));
    let secp = Secp256k1::new();
    let master = ExtendedPrivKey::new_master(network, &seed)
        .context("failed to derive master xprv from seed")?;
    let coin_type = match network {
        Network::Bitcoin => 0,
        _ => 1,
    };
    let account_path = DerivationPath::from_str(&format!("m/84'/{}'/0'", coin_type))
        .map_err(|_| anyhow!("invalid derivation path"))?;
    let account_xprv = master
        .derive_priv(&secp, &account_path)
        .context("failed to derive account xprv")?;
    let account_xpub = ExtendedPubKey::from_priv(&secp, &account_xprv);
    let fingerprint = master.fingerprint(&secp);
    let origin = format!("[{}/84h/{}h/0h]", fingerprint, coin_type);

    Ok(SeedDerivedDescriptors {
        public_descriptor: format!("wpkh({}{}/0/*)", origin, account_xpub),
        spend_descriptor: format!("wpkh({}{}/0/*)", origin, account_xprv),
        change_descriptor: format!("wpkh({}{}/1/*)", origin, account_xprv),
    })
}
