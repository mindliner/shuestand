use anyhow::Context;
use async_trait::async_trait;
use axum::http::Method;
use backend::address::{AddressFactory, AddressPool};
use backend::config::AppConfig;
use backend::db::{Database, DepositState, WithdrawalState};
use backend::fees::FeeEstimator;
use backend::onchain::{BlockchainClient, OnchainWallet, TxStatus};
use backend::operations::OperationMode;
use backend::telemetry::AppMetrics;
use backend::transactions::TransactionNotifier;
use backend::wallet::{
    MintSwapService, MultiMintWalletManager, WalletConfig, WalletHandle, open_wallet,
};
use backend::workers::deposit::{CashuTokenSender, DepositTokenSender, DepositWorker};
use backend::workers::withdrawal::{
    CashuRedeemer, CashuRedeemingExecutor, CashuToOnchainExecutor, CdkCashuRedeemer,
    MockWithdrawalExecutor, WithdrawalExecutor, WithdrawalWorker,
};
use bdk::bitcoin::{Address, Network};
use cdk::mint_url::MintUrl;
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use electrum_client::{
    Client as ElectrumRpcClient, ConfigBuilder as ElectrumConfigBuilder, ElectrumApi,
    Socks5Config as ElectrumSocks5Config,
};
use reqwest::{Client, StatusCode as HttpStatus};
use serde::Deserialize;
use serde_json::json;
use sqlx::Error;
use std::{
    net::SocketAddr,
    path::PathBuf,
    str::FromStr,
    sync::{Arc, Mutex},
    time::Duration,
};
use thiserror::Error;
use tokio::sync::RwLock;
use tokio::{task::spawn_blocking, time::sleep};

mod http;

use tower_http::cors::{Any, CorsLayer};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
use urlencoding::encode as url_encode;

const MAX_DEPOSIT_SATS: u64 = 2_000_000;
const ADDRESS_POOL_REFILL_INTERVAL_SECS: u64 = 60;
const ELECTRUM_RETRY: u8 = 5;
const ELECTRUM_TIMEOUT_SECS: u8 = 30;
const PREDEPOSIT_TX_TOLERANCE_SECS: i64 = 600; // 10 minutes
const DEFAULT_SESSION_TTL_HOURS: i64 = 8;

#[derive(Clone, Copy, PartialEq)]
enum FloatBand {
    Unknown,
    Ok,
    Low,
    High,
}

#[derive(Clone)]
struct WalletFloatStatus {
    balance_sats: u64,
    ratio: f32,
    state: FloatBand,
    updated_at: Option<DateTime<Utc>>,
}

impl WalletFloatStatus {
    fn unknown() -> Self {
        Self {
            balance_sats: 0,
            ratio: 0.0,
            state: FloatBand::Unknown,
            updated_at: None,
        }
    }
}

impl Default for WalletFloatStatus {
    fn default() -> Self {
        Self::unknown()
    }
}

#[derive(Clone)]
struct FloatStatus {
    onchain: WalletFloatStatus,
    cashu: WalletFloatStatus,
}

impl Default for FloatStatus {
    fn default() -> Self {
        Self {
            onchain: WalletFloatStatus::unknown(),
            cashu: WalletFloatStatus::unknown(),
        }
    }
}

impl FloatBand {
    fn as_str(&self) -> &'static str {
        match self {
            FloatBand::Unknown => "unknown",
            FloatBand::Ok => "ok",
            FloatBand::Low => "low",
            FloatBand::High => "high",
        }
    }
}

#[derive(Clone)]
struct AppState {
    db: Database,
    address_pool: AddressPool,
    deposit_target_confirmations: u8,
    onchain_wallet: Option<Arc<OnchainWallet>>,
    wallet_api_token: Option<String>,
    metrics: Arc<AppMetrics>,
    cashu_wallet: Option<WalletHandle>,
    cashu_mint_url: Option<String>,
    public_base_url: Option<String>,
    float_status: Arc<RwLock<FloatStatus>>,
    operation_mode: Arc<RwLock<OperationMode>>,
    float_target_sats: u64,
    float_min_ratio: f32,
    float_max_ratio: f32,
    deposit_min_sats: u64,
    withdrawal_min_sats: u64,
    single_request_cap_ratio: f64,
    session_ttl: ChronoDuration,
    transaction_notifier: Option<Arc<TransactionNotifier>>,
    security_alert_webhook_url: Option<String>,
    fee_estimator: Arc<FeeEstimator>,
}

impl AppState {
    pub(crate) async fn current_operation_mode(&self) -> OperationMode {
        *self.operation_mode.read().await
    }

    pub(crate) async fn set_operation_mode(&self, next: OperationMode) -> Result<(), Error> {
        {
            let guard = self.operation_mode.read().await;
            if *guard == next {
                return Ok(());
            }
        }
        self.db.persist_operation_mode(next).await?;
        let mut guard = self.operation_mode.write().await;
        *guard = next;
        Ok(())
    }
}

#[tokio::main]
async fn main() -> Result<(), anyhow::Error> {
    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::new(
            std::env::var("RUST_LOG").unwrap_or_else(|_| "backend=debug".into()),
        ))
        .with(tracing_subscriber::fmt::layer())
        .init();

    let config = AppConfig::from_env();

    let database_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://postgres:postgres@localhost/shuestand".into());

    let db = Database::connect(&database_url).await?;
    tracing::info!(target: "backend", "connected to database");

    let transaction_notifier =
        TransactionNotifier::maybe_new(db.clone(), config.transaction_webhook_url.clone())
            .map(Arc::new);

    let initial_operation_mode = match db.load_operation_mode().await {
        Ok(mode) => mode,
        Err(err) => {
            tracing::warn!(
                target: "backend",
                error = %err,
                "failed to load operation mode; defaulting to normal"
            );
            OperationMode::Normal
        }
    };
    let operation_mode = Arc::new(RwLock::new(initial_operation_mode));

    let metrics = Arc::new(AppMetrics::new());

    let fee_estimator = Arc::new(FeeEstimator::new(config.fee_estimator.clone()));
    if fee_estimator.has_remote() {
        if let Err(err) = fee_estimator.refresh().await {
            tracing::warn!(
                target = "backend",
                error = %err,
                "initial fee estimator refresh failed"
            );
        }

        let estimator = fee_estimator.clone();
        let refresh_interval = config.fee_estimator.refresh_interval;
        tokio::spawn(async move {
            loop {
                if let Err(err) = estimator.refresh().await {
                    tracing::warn!(
                        target = "backend",
                        error = %err,
                        "fee estimator refresh failed"
                    );
                }
                sleep(refresh_interval).await;
            }
        });
    }
    let float_status = Arc::new(RwLock::new(FloatStatus::default()));

    let address_factory =
        AddressFactory::from_config(config.bitcoin_descriptor.clone(), config.bitcoin_network)?;
    let address_pool = AddressPool::new(db.clone(), address_factory, config.address_pool_target);
    address_pool.ensure_capacity().await?;

    {
        let pool_clone = address_pool.clone();
        tokio::spawn(async move {
            loop {
                if let Err(err) = pool_clone.ensure_capacity().await {
                    tracing::error!(target: "backend", error = %err, "address pool refill failed");
                }
                sleep(Duration::from_secs(ADDRESS_POOL_REFILL_INTERVAL_SECS)).await;
            }
        });
    }

    let chain_source: Option<Arc<dyn ChainSource>> =
        if let Some(url) = config.bitcoin_electrum_url.clone() {
            match ElectrumChainSource::connect(
                url.as_str(),
                config.bitcoin_electrum_socks5.as_deref(),
                config.bitcoin_electrum_validate_domain,
                config.bitcoin_network,
            ) {
                Ok(source) => Some(Arc::new(source)),
                Err(err) => {
                    tracing::error!(
                        target: "backend",
                        error = %err,
                        "failed to initialize electrum chain source"
                    );
                    None
                }
            }
        } else if let Some(base_url) = config.esplora_base_url.clone() {
            Some(Arc::new(EsploraClient::new(base_url)))
        } else {
            None
        };

    if let Some(chain) = chain_source.clone() {
        let watcher_db = db.clone();
        let poll_every = config.confirmation_poll_interval;
        let mode_handle = operation_mode.clone();
        tokio::spawn(async move {
            loop {
                if matches!(*mode_handle.read().await, OperationMode::Halt) {
                    sleep(poll_every).await;
                    continue;
                }
                if let Err(err) = process_confirmations(&watcher_db, chain.as_ref()).await {
                    tracing::error!(target: "backend", error = %err, "confirmation pass failed");
                }
                sleep(poll_every).await;
            }
        });
    } else {
        tracing::warn!(
            target: "backend",
            "no chain source configured; deposit confirmations disabled"
        );
    }

    let blockchain_client = if let Some(url) = config.bitcoin_electrum_url.as_deref() {
        match BlockchainClient::from_electrum_config(
            url,
            config.bitcoin_electrum_socks5.as_deref(),
            ELECTRUM_RETRY,
            Some(ELECTRUM_TIMEOUT_SECS),
            config.bitcoin_electrum_validate_domain,
        ) {
            Ok(client) => Some(client),
            Err(err) => {
                tracing::error!(
                    target: "backend",
                    error = %err,
                    "failed to initialize electrum blockchain client"
                );
                None
            }
        }
    } else if let Some(esplora_base) = config.esplora_base_url.as_deref() {
        Some(BlockchainClient::from_esplora(esplora_base))
    } else {
        None
    };

    let onchain_wallet = if let (Some(spend), Some(blockchain)) = (
        config.bitcoin_spend_descriptor.clone(),
        blockchain_client.clone(),
    ) {
        match OnchainWallet::new(
            &db,
            spend.as_str(),
            config.bitcoin_change_descriptor.as_deref(),
            config.bitcoin_network,
            blockchain,
        )
        .await
        {
            Ok(wallet) => {
                tracing::info!(target: "backend", "on-chain wallet initialized");
                Some(Arc::new(wallet))
            }
            Err(err) => {
                tracing::error!(
                    target: "backend",
                    error = %err,
                    "failed to initialize on-chain wallet"
                );
                None
            }
        }
    } else {
        if config.bitcoin_spend_descriptor.is_some() {
            tracing::warn!(
                target: "backend",
                "on-chain wallet descriptors configured but no blockchain backend available"
            );
        }
        None
    };

    if let Some(wallet) = &onchain_wallet {
        if let Err(err) = wallet.sync().await {
            tracing::warn!(
                target: "backend",
                error = %err,
                "failed to sync on-chain wallet on startup"
            );
        }
    }

    let normalized_cashu_mint = match config.cashu_mint_url.as_deref() {
        Some(url) => match MintUrl::from_str(url) {
            Ok(mint) => Some(mint.to_string()),
            Err(err) => {
                tracing::warn!(
                    target: "backend",
                    error = %err,
                    "invalid CASHU_MINT_URL; mint validation disabled"
                );
                None
            }
        },
        None => None,
    };

    let cashu_wallet_requested = config.cashu_mint_url.is_some();
    let mut cashu_wallet_base_dir: Option<PathBuf> = None;
    let cashu_wallet = if let Some(mint_url) = config.cashu_mint_url.as_deref() {
        let wallet_config =
            WalletConfig::new(mint_url.to_string(), config.cashu_wallet_dir.clone());
        let base_dir = wallet_config.base_dir();
        match open_wallet(&wallet_config).await {
            Ok(wallet) => {
                cashu_wallet_base_dir = Some(base_dir);
                tracing::info!(target: "backend", mint = mint_url, "cashu wallet initialized");
                Some(wallet)
            }
            Err(err) => {
                tracing::error!(
                    target: "backend",
                    error = %err,
                    "failed to initialize cashu wallet"
                );
                None
            }
        }
    } else {
        None
    };

    let cashu_wallet_manager = if let (Some(wallet), Some(base_dir)) =
        (cashu_wallet.clone(), cashu_wallet_base_dir.clone())
    {
        if let Some(canonical_mint) = normalized_cashu_mint
            .clone()
            .or_else(|| config.cashu_mint_url.clone())
        {
            Some(Arc::new(MultiMintWalletManager::new(
                canonical_mint,
                wallet,
                base_dir,
            )))
        } else {
            None
        }
    } else {
        None
    };

    let mint_swap_service = cashu_wallet_manager
        .as_ref()
        .map(|manager| Arc::new(MintSwapService::new(manager.clone())));

    {
        let onchain = onchain_wallet.clone();
        let cashu = cashu_wallet.clone();
        let metrics_clone = metrics.clone();
        let status = float_status.clone();
        let target = config.float_target_sats;
        let min_ratio = config.float_min_ratio;
        let max_ratio = config.float_max_ratio;
        let interval = config.float_guard_interval;
        let drift_alert_ratio = config.float_drift_alert_ratio;
        let mode_handle = operation_mode.clone();
        let db_clone = db.clone();
        let auto_drain_threshold = config.withdrawal_min_sats.saturating_mul(2);
        let alert_webhook = config.float_alert_webhook_url.clone();

        tokio::spawn(async move {
            monitor_float_levels(
                onchain,
                cashu,
                metrics_clone,
                status,
                target,
                min_ratio,
                max_ratio,
                drift_alert_ratio,
                interval,
                auto_drain_threshold,
                mode_handle,
                db_clone,
                alert_webhook,
            )
            .await;
        });
    }

    if config.withdrawal_worker_enabled {
        tracing::info!(
            target: "backend",
            interval_secs = config.withdrawal_worker_interval.as_secs(),
            "withdrawal worker enabled"
        );
        let executor: Arc<dyn WithdrawalExecutor + Send + Sync> =
            if let (Some(manager), Some(onchain_wallet), Some(swapper)) = (
                cashu_wallet_manager.clone(),
                onchain_wallet.clone(),
                mint_swap_service.clone(),
            ) {
                tracing::info!(
                    target: "backend",
                    fee_rate_vb = config.withdrawal_payout_fee_rate_vb,
                    "cashu redeemer + on-chain payout enabled"
                );
                let redeemer =
                    Arc::new(CdkCashuRedeemer::new(manager, swapper)) as Arc<dyn CashuRedeemer>;
                Arc::new(CashuToOnchainExecutor::new(
                    redeemer,
                    onchain_wallet,
                    fee_estimator.clone(),
                    db.clone(),
                ))
            } else if let (Some(manager), Some(swapper)) =
                (cashu_wallet_manager.clone(), mint_swap_service.clone())
            {
                tracing::info!(target: "backend", "cashu redeemer enabled (no on-chain wallet)");
                let redeemer =
                    Arc::new(CdkCashuRedeemer::new(manager, swapper)) as Arc<dyn CashuRedeemer>;
                Arc::new(CashuRedeemingExecutor {
                    redeemer,
                    db: db.clone(),
                })
            } else {
                if cashu_wallet_requested {
                    tracing::warn!(
                        target: "backend",
                        "cashu wallet unavailable; using mock withdrawal executor"
                    );
                } else {
                    tracing::warn!(
                        target: "backend",
                        "CASHU_MINT_URL not set; using mock withdrawal executor"
                    );
                }
                Arc::new(MockWithdrawalExecutor)
            };

        let worker = WithdrawalWorker::new(
            db.clone(),
            executor,
            config.withdrawal_worker_interval,
            config.withdrawal_worker_max_attempts,
            metrics.clone(),
            operation_mode.clone(),
            transaction_notifier.clone(),
        );
        tokio::spawn(async move {
            worker.run().await;
        });
    } else {
        tracing::info!(target: "backend", "withdrawal worker disabled via config");
    }

    if config.deposit_worker_enabled {
        tracing::info!(
            target: "backend",
            interval_secs = config.deposit_worker_interval.as_secs(),
            "deposit worker enabled"
        );
        if let Some(wallet) = cashu_wallet.clone() {
            let sender: Arc<dyn DepositTokenSender + Send + Sync> =
                Arc::new(CashuTokenSender::new(wallet));
            let worker = DepositWorker::new(
                db.clone(),
                sender,
                config.deposit_worker_interval,
                config.deposit_worker_max_attempts,
                Client::new(),
                operation_mode.clone(),
                transaction_notifier.clone(),
            );
            tokio::spawn(async move {
                worker.run().await;
            });
        } else {
            tracing::warn!(
                target: "backend",
                "deposit worker enabled but CASHU_MINT_URL is missing or wallet init failed"
            );
        }
    } else {
        tracing::info!(target: "backend", "deposit worker disabled via config");
    }

    if let Some(wallet) = onchain_wallet.clone() {
        let watcher_db = db.clone();
        let poll_every = config.confirmation_poll_interval;
        let target_conf = config.withdrawal_target_confirmations;
        let mode_handle = operation_mode.clone();
        let tx_notifier = transaction_notifier.clone();
        tokio::spawn(async move {
            loop {
                if matches!(*mode_handle.read().await, OperationMode::Halt) {
                    sleep(poll_every).await;
                    continue;
                }
                if let Err(err) = process_withdrawal_settlements(
                    &watcher_db,
                    wallet.clone(),
                    target_conf,
                    tx_notifier.clone(),
                )
                .await
                {
                    tracing::error!(target: "backend", error = %err, "withdrawal confirmation pass failed");
                }
                sleep(poll_every).await;
            }
        });
    }

    let cors = CorsLayer::new()
        .allow_methods([Method::GET, Method::POST])
        .allow_headers(Any)
        .allow_origin(Any);

    let state = AppState {
        db: db.clone(),
        address_pool,
        deposit_target_confirmations: config.deposit_target_confirmations,
        onchain_wallet: onchain_wallet.clone(),
        wallet_api_token: config.wallet_api_token.clone(),
        metrics: metrics.clone(),
        cashu_wallet: cashu_wallet.clone(),
        cashu_mint_url: normalized_cashu_mint.clone(),
        public_base_url: config.public_base_url.clone(),
        float_status: float_status.clone(),
        operation_mode: operation_mode.clone(),
        float_target_sats: config.float_target_sats,
        float_min_ratio: config.float_min_ratio,
        float_max_ratio: config.float_max_ratio,
        deposit_min_sats: config.deposit_min_sats,
        withdrawal_min_sats: config.withdrawal_min_sats,
        single_request_cap_ratio: config.single_request_cap_ratio,
        session_ttl: ChronoDuration::hours(DEFAULT_SESSION_TTL_HOURS),
        transaction_notifier: transaction_notifier.clone(),
        security_alert_webhook_url: config.security_alert_webhook_url.clone(),
        fee_estimator: fee_estimator.clone(),
    };

    let app = http::router(state).layer(cors);

    let port = std::env::var("SHUESTAND_BACKEND_PORT")
        .ok()
        .and_then(|v| v.parse::<u16>().ok())
        .unwrap_or(8080);
    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    tracing::info!(%addr, "shuestand backend listening");

    axum::serve(tokio::net::TcpListener::bind(addr).await?, app).await?;

    Ok(())
}

async fn monitor_float_levels(
    onchain_wallet: Option<Arc<OnchainWallet>>,
    cashu_wallet: Option<WalletHandle>,
    metrics: Arc<AppMetrics>,
    status: Arc<RwLock<FloatStatus>>,
    target_sats: u64,
    min_ratio: f32,
    max_ratio: f32,
    drift_alert_ratio: f32,
    interval: Duration,
    auto_drain_threshold: u64,
    operation_mode: Arc<RwLock<OperationMode>>,
    db: Database,
    alert_webhook_url: Option<String>,
) {
    if target_sats == 0 {
        tracing::warn!(target: "backend", "FLOAT_TARGET_SATS is zero; float guard disabled");
        return;
    }

    let mut drift_alert_active = false;
    let mut last_total_balance: Option<u64> = None;

    loop {
        let onchain_snapshot =
            compute_onchain_float(onchain_wallet.as_ref(), target_sats, min_ratio, max_ratio).await;
        metrics.set_onchain_float_ratio(onchain_snapshot.ratio as f64);

        let cashu_snapshot =
            compute_cashu_float(cashu_wallet.as_ref(), target_sats, min_ratio, max_ratio).await;
        metrics.set_cashu_float_ratio(cashu_snapshot.ratio as f64);

        let mut onchain_transition: Option<FloatBand> = None;
        let mut cashu_transition: Option<FloatBand> = None;
        {
            let mut guard = status.write().await;
            if onchain_snapshot.state != guard.onchain.state {
                log_float_transition(
                    "onchain",
                    guard.onchain.state,
                    onchain_snapshot.state,
                    onchain_snapshot.balance_sats,
                    target_sats,
                );
                onchain_transition = Some(onchain_snapshot.state);
            }
            if cashu_snapshot.state != guard.cashu.state {
                log_float_transition(
                    "cashu",
                    guard.cashu.state,
                    cashu_snapshot.state,
                    cashu_snapshot.balance_sats,
                    target_sats,
                );
                cashu_transition = Some(cashu_snapshot.state);
            }
            guard.onchain = onchain_snapshot.clone();
            guard.cashu = cashu_snapshot.clone();
        }

        if let (Some(url), Some(state)) = (alert_webhook_url.as_deref(), onchain_transition) {
            if let Err(err) = emit_float_band_alert(
                url,
                "onchain",
                state,
                onchain_snapshot.balance_sats,
                target_sats,
            )
            .await
            {
                tracing::warn!(
                    target: "backend",
                    error = %err,
                    "failed to send on-chain float alert"
                );
            }
        }
        if let (Some(url), Some(state)) = (alert_webhook_url.as_deref(), cashu_transition) {
            if let Err(err) = emit_float_band_alert(
                url,
                "cashu",
                state,
                cashu_snapshot.balance_sats,
                target_sats,
            )
            .await
            {
                tracing::warn!(
                    target: "backend",
                    error = %err,
                    "failed to send cashu float alert"
                );
            }
        }

        let total_balance = onchain_snapshot.balance_sats + cashu_snapshot.balance_sats;
        let drift_vs_target = target_sats as i64 - total_balance as i64;
        if target_sats > 0 {
            let total_ratio = total_balance as f64 / target_sats as f64;
            metrics.set_total_float_ratio(total_ratio);
            metrics.set_float_drift_sats(drift_vs_target);

            if let Some(previous_total) = last_total_balance {
                let delta = total_balance as i64 - previous_total as i64;
                let delta_ratio = (delta.abs() as f64) / target_sats as f64;

                if delta_ratio >= drift_alert_ratio as f64 {
                    if !drift_alert_active {
                        tracing::warn!(
                            target: "backend",
                            total_balance_sats = total_balance,
                            previous_total_sats = previous_total,
                            target_sats,
                            delta_sats = delta,
                            "total float change exceeded threshold"
                        );
                        drift_alert_active = true;
                        if let Some(url) = alert_webhook_url.as_deref() {
                            if let Err(err) = emit_float_drift_alert(
                                url,
                                "triggered",
                                total_balance,
                                target_sats,
                                drift_vs_target,
                                delta,
                                previous_total,
                            )
                            .await
                            {
                                tracing::warn!(
                                    target: "backend",
                                    error = %err,
                                    "failed to send drift alert"
                                );
                            }
                        }
                    }
                } else if drift_alert_active {
                    tracing::info!(
                        target: "backend",
                        total_balance_sats = total_balance,
                        previous_total_sats = previous_total,
                        target_sats,
                        "total float change back within buffer"
                    );
                    drift_alert_active = false;
                    if let Some(url) = alert_webhook_url.as_deref() {
                        if let Err(err) = emit_float_drift_alert(
                            url,
                            "cleared",
                            total_balance,
                            target_sats,
                            drift_vs_target,
                            delta,
                            previous_total,
                        )
                        .await
                        {
                            tracing::warn!(
                                target: "backend",
                                error = %err,
                                "failed to send drift recovery alert"
                            );
                        }
                    }
                }
            }
        }

        last_total_balance = Some(total_balance);

        if auto_drain_threshold > 0 && total_balance <= auto_drain_threshold {
            let should_trigger = {
                let guard = operation_mode.read().await;
                *guard == OperationMode::Normal
            };
            if should_trigger {
                match db.persist_operation_mode(OperationMode::Drain).await {
                    Ok(_) => {
                        let mut guard = operation_mode.write().await;
                        if *guard == OperationMode::Normal {
                            *guard = OperationMode::Drain;
                            tracing::warn!(
                                target: "backend",
                                total_balance_sats = total_balance,
                                threshold_sats = auto_drain_threshold,
                                "total float hit the auto-drain threshold; switching to drain mode"
                            );
                        }
                    }
                    Err(err) => {
                        tracing::error!(
                            target: "backend",
                            error = %err,
                            "failed to persist auto-drain mode"
                        );
                    }
                }
            }
        }

        sleep(interval).await;
    }
}

async fn compute_onchain_float(
    wallet: Option<&Arc<OnchainWallet>>,
    target_sats: u64,
    min_ratio: f32,
    max_ratio: f32,
) -> WalletFloatStatus {
    let mut status = WalletFloatStatus::unknown();
    if let Some(wallet) = wallet {
        match wallet.balance().await {
            Ok(summary) => {
                let balance_sats = summary.confirmed + summary.trusted_pending;
                let ratio = compute_ratio(balance_sats, target_sats);
                status.balance_sats = balance_sats;
                status.ratio = ratio;
                status.state = classify_ratio(ratio, min_ratio, max_ratio);
                status.updated_at = Some(Utc::now());
            }
            Err(err) => {
                tracing::warn!(
                    target: "backend",
                    error = %err,
                    "failed to read on-chain wallet balance for float guard"
                );
            }
        }
    }
    status
}

async fn compute_cashu_float(
    wallet: Option<&WalletHandle>,
    target_sats: u64,
    min_ratio: f32,
    max_ratio: f32,
) -> WalletFloatStatus {
    let mut status = WalletFloatStatus::unknown();
    if let Some(handle) = wallet {
        let guard = handle.lock().await;
        match guard.total_balance().await {
            Ok(amount) => {
                let balance_sats = amount.to_u64();
                let ratio = compute_ratio(balance_sats, target_sats);
                status.balance_sats = balance_sats;
                status.ratio = ratio;
                status.state = classify_ratio(ratio, min_ratio, max_ratio);
                status.updated_at = Some(Utc::now());
            }
            Err(err) => {
                tracing::warn!(
                    target: "backend",
                    error = %err,
                    "failed to read cashu wallet balance for float guard"
                );
            }
        }
    }
    status
}

fn compute_ratio(balance_sats: u64, target_sats: u64) -> f32 {
    if target_sats == 0 {
        0.0
    } else {
        balance_sats as f32 / target_sats as f32
    }
}

fn classify_ratio(ratio: f32, min_ratio: f32, max_ratio: f32) -> FloatBand {
    if ratio <= 0.0 {
        return FloatBand::Low;
    }
    if ratio < min_ratio {
        FloatBand::Low
    } else if ratio > max_ratio {
        FloatBand::High
    } else {
        FloatBand::Ok
    }
}

fn log_float_transition(
    wallet: &str,
    previous: FloatBand,
    current: FloatBand,
    balance_sats: u64,
    target_sats: u64,
) {
    if current == previous {
        return;
    }
    let ratio = compute_ratio(balance_sats, target_sats);
    match current {
        FloatBand::Low => tracing::warn!(
            target: "backend",
            wallet,
            balance_sats,
            target_sats,
            ratio,
            "float below minimum threshold"
        ),
        FloatBand::High => tracing::warn!(
            target: "backend",
            wallet,
            balance_sats,
            target_sats,
            ratio,
            "float above maximum threshold"
        ),
        FloatBand::Ok => tracing::info!(
            target: "backend",
            wallet,
            balance_sats,
            target_sats,
            ratio,
            "float back within guard rails"
        ),
        FloatBand::Unknown => tracing::warn!(
            target: "backend",
            wallet,
            "float status unknown"
        ),
    }
}

async fn emit_float_band_alert(
    url: &str,
    wallet: &str,
    state: FloatBand,
    balance_sats: u64,
    target_sats: u64,
) -> Result<(), reqwest::Error> {
    let payload = json!({
        "event": "float_band_change",
        "wallet": wallet,
        "state": state.as_str(),
        "balance_sats": balance_sats,
        "target_sats": target_sats,
    });
    post_float_alert(url, payload).await
}

async fn emit_float_drift_alert(
    url: &str,
    state: &str,
    total_balance_sats: u64,
    target_sats: u64,
    drift_sats: i64,
    delta_sats: i64,
    previous_total_sats: u64,
) -> Result<(), reqwest::Error> {
    let payload = json!({
        "event": "float_drift_alert",
        "state": state,
        "total_balance_sats": total_balance_sats,
        "target_sats": target_sats,
        "drift_sats": drift_sats,
        "delta_sats": delta_sats,
        "previous_total_sats": previous_total_sats,
    });
    post_float_alert(url, payload).await
}

async fn post_float_alert(url: &str, payload: serde_json::Value) -> Result<(), reqwest::Error> {
    Client::new()
        .post(url)
        .json(&payload)
        .send()
        .await?
        .error_for_status()
        .map(|_| ())
}

async fn process_confirmations(db: &Database, chain: &dyn ChainSource) -> Result<(), TrackerError> {
    let deposits = db.list_open_deposits().await?;
    if deposits.is_empty() {
        return Ok(());
    }
    let tip_height = chain.tip_height().await?;

    for deposit in deposits {
        if let Some(observation) = chain.first_matching_tx(&deposit.address).await? {
            if observation.confirmed {
                if let Some(seen_at) = observation.seen_at {
                    if seen_at + ChronoDuration::seconds(PREDEPOSIT_TX_TOLERANCE_SECS)
                        < deposit.created_at
                    {
                        tracing::info!(
                            target: "backend",
                            deposit_id = %deposit.id,
                            txid = %observation.txid,
                            "ignoring transaction confirmed before deposit was created"
                        );
                        continue;
                    }
                }
            }

            let confirmations = if observation.confirmed {
                match observation.block_height {
                    Some(height) if tip_height >= height => tip_height - height + 1,
                    Some(_) => 1,
                    None => 1,
                }
            } else {
                0
            };

            let new_state = if confirmations >= deposit.target_confirmations as u32 {
                DepositState::Minting
            } else {
                DepositState::Confirming
            };

            db.update_deposit_chain_state(&deposit.id, &observation.txid, confirmations, new_state)
                .await?;
            db.update_address_observation(&deposit.id, &observation.txid, confirmations)
                .await?;
        }
    }

    Ok(())
}

async fn process_withdrawal_settlements(
    db: &Database,
    wallet: Arc<OnchainWallet>,
    target_confirmations: u8,
    transaction_notifier: Option<Arc<TransactionNotifier>>,
) -> Result<(), anyhow::Error> {
    let tracked = db
        .list_withdrawals_by_state(&[WithdrawalState::Broadcasting, WithdrawalState::Confirming])
        .await?;
    if tracked.is_empty() {
        return Ok(());
    }

    if let Err(err) = wallet.sync().await {
        tracing::warn!(target: "backend", error = %err, "on-chain wallet sync failed during withdrawal watcher");
    }

    for withdrawal in tracked {
        let txid = match withdrawal.txid.as_deref() {
            Some(value) => value,
            None => continue,
        };

        match wallet.tx_status(txid).await? {
            None => {
                tracing::debug!(target: "backend", withdrawal_id = %withdrawal.id, "withdrawal txid not yet visible in wallet");
            }
            Some(TxStatus::Pending) => {
                if withdrawal.state == WithdrawalState::Broadcasting {
                    db.update_withdrawal_chain_state(&withdrawal.id, WithdrawalState::Confirming)
                        .await?;
                    tracing::info!(target: "backend", withdrawal_id = %withdrawal.id, txid = txid, "withdrawal broadcast observed in mempool");
                }
            }
            Some(TxStatus::Confirmed) => {
                if withdrawal.state != WithdrawalState::Settled {
                    db.update_withdrawal_chain_state(&withdrawal.id, WithdrawalState::Settled)
                        .await?;
                    tracing::info!(target: "backend", withdrawal_id = %withdrawal.id, txid = txid, target_confirmations = target_confirmations, "withdrawal reached confirmation target");
                    if let Some(notifier) = transaction_notifier.as_ref() {
                        notifier.record_withdrawal(&withdrawal.id).await;
                    }
                }
            }
        }
    }

    Ok(())
}

#[async_trait]
trait ChainSource: Send + Sync {
    async fn tip_height(&self) -> anyhow::Result<u32>;
    async fn first_matching_tx(&self, address: &str) -> anyhow::Result<Option<ObservedTx>>;
}

struct EsploraClient {
    http: Client,
    base_url: String,
}

impl EsploraClient {
    fn new(base_url: String) -> Self {
        Self {
            http: Client::new(),
            base_url,
        }
    }
}

struct ElectrumChainSource {
    client: Arc<Mutex<ElectrumRpcClient>>,
    network: Network,
}

impl ElectrumChainSource {
    fn connect(
        url: &str,
        socks5: Option<&str>,
        validate_domain: bool,
        network: Network,
    ) -> anyhow::Result<Self> {
        let socks5_cfg = socks5.map(ElectrumSocks5Config::new);
        let config = ElectrumConfigBuilder::new()
            .retry(ELECTRUM_RETRY)
            .timeout(Some(ELECTRUM_TIMEOUT_SECS))
            .socks5(socks5_cfg)
            .validate_domain(validate_domain)
            .build();
        let client = ElectrumRpcClient::from_config(url, config)?;
        Ok(Self {
            client: Arc::new(Mutex::new(client)),
            network,
        })
    }
}

#[derive(Debug, Deserialize)]
struct EsploraTx {
    txid: String,
    status: EsploraStatus,
    vout: Vec<EsploraVout>,
}

#[derive(Debug, Deserialize)]
struct EsploraStatus {
    confirmed: bool,
    block_height: Option<u32>,
    block_time: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct EsploraVout {
    #[serde(default)]
    scriptpubkey_address: Option<String>,
}

#[derive(Clone, Debug)]
struct ObservedTx {
    txid: String,
    confirmed: bool,
    block_height: Option<u32>,
    seen_at: Option<DateTime<Utc>>,
}

fn unix_seconds_to_datetime(ts: u64) -> Option<DateTime<Utc>> {
    DateTime::<Utc>::from_timestamp(ts as i64, 0)
}

fn tx_rank(confirmed: bool, block_height: Option<u32>) -> u64 {
    if !confirmed {
        return u64::MAX;
    }
    block_height.map(|h| h as u64).unwrap_or(0)
}

fn is_newer(candidate: &ObservedTx, current: &ObservedTx) -> bool {
    match (candidate.seen_at, current.seen_at) {
        (Some(cand), Some(curr)) => cand > curr,
        (Some(_), None) => true,
        _ => false,
    }
}

#[async_trait]
impl ChainSource for EsploraClient {
    async fn tip_height(&self) -> anyhow::Result<u32> {
        let url = format!("{}/blocks/tip/height", self.base_url);
        let text = self
            .http
            .get(url)
            .send()
            .await?
            .error_for_status()?
            .text()
            .await?;
        Ok(text.trim().parse::<u32>()?)
    }

    async fn first_matching_tx(&self, address: &str) -> anyhow::Result<Option<ObservedTx>> {
        let url = format!("{}/address/{}/txs", self.base_url, url_encode(address));
        let resp = self.http.get(url).send().await?;
        if resp.status() == HttpStatus::NOT_FOUND {
            return Ok(None);
        }
        let txs: Vec<EsploraTx> = resp.error_for_status()?.json().await?;
        let mut best: Option<(ObservedTx, u64)> = None;
        for tx in txs {
            let matches = tx
                .vout
                .iter()
                .any(|v| v.scriptpubkey_address.as_deref() == Some(address));
            if !matches {
                continue;
            }
            let seen_at = if tx.status.confirmed {
                tx.status.block_time.and_then(unix_seconds_to_datetime)
            } else {
                None
            };
            let candidate = ObservedTx {
                txid: tx.txid,
                confirmed: tx.status.confirmed,
                block_height: tx.status.block_height,
                seen_at,
            };
            let rank = tx_rank(tx.status.confirmed, tx.status.block_height);
            match &mut best {
                None => best = Some((candidate, rank)),
                Some((best_tx, best_rank)) => {
                    if rank > *best_rank || (rank == *best_rank && is_newer(&candidate, best_tx)) {
                        *best_tx = candidate;
                        *best_rank = rank;
                    }
                }
            }
        }
        Ok(best.map(|(tx, _)| tx))
    }
}

#[async_trait]
impl ChainSource for ElectrumChainSource {
    async fn tip_height(&self) -> anyhow::Result<u32> {
        let client = self.client.clone();
        let height = spawn_blocking(move || {
            let guard = client.lock().expect("electrum client poisoned");
            guard
                .block_headers_subscribe()
                .map(|header| header.height as u32)
                .map_err(anyhow::Error::from)
        })
        .await??;
        Ok(height)
    }

    async fn first_matching_tx(&self, address: &str) -> anyhow::Result<Option<ObservedTx>> {
        let parsed = Address::from_str(address)?
            .require_network(self.network)
            .context("address network mismatch")?;
        let script = parsed.script_pubkey();
        let client = self.client.clone();
        let entry = spawn_blocking(
            move || -> Result<Option<electrum_client::GetHistoryRes>, electrum_client::Error> {
                let mut history = client
                    .lock()
                    .expect("electrum client poisoned")
                    .script_get_history(&script)?;
                Ok(history.pop())
            },
        )
        .await??;

        let Some(item) = entry else {
            return Ok(None);
        };
        let confirmed = item.height > 0;
        let block_height = if confirmed {
            Some(item.height as u32)
        } else {
            None
        };
        let seen_at = if confirmed {
            let height = item.height as usize;
            let client = self.client.clone();
            let header = spawn_blocking(move || {
                client
                    .lock()
                    .expect("electrum client poisoned")
                    .block_header(height)
            })
            .await??;
            unix_seconds_to_datetime(header.time as u64)
        } else {
            None
        };
        Ok(Some(ObservedTx {
            txid: item.tx_hash.to_string(),
            confirmed,
            block_height,
            seen_at,
        }))
    }
}

#[derive(Error, Debug)]
enum TrackerError {
    #[error("chain source error: {0}")]
    Chain(#[from] anyhow::Error),
    #[error("database error: {0}")]
    Database(#[from] Error),
}
