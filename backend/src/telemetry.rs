use prometheus_client::encoding::text::encode;
use prometheus_client::metrics::counter::Counter;
use prometheus_client::metrics::gauge::Gauge;
use prometheus_client::registry::Registry;

pub struct AppMetrics {
    registry: Registry,
    wallet_sync_total: Counter,
    wallet_sync_failures: Counter,
    withdrawal_attempt_total: Counter,
    withdrawal_failure_total: Counter,
    onchain_float_ratio: Gauge<i64>,
    cashu_float_ratio: Gauge<i64>,
    total_float_ratio: Gauge<i64>,
    float_drift_sats: Gauge<i64>,
}

impl AppMetrics {
    pub fn new() -> Self {
        let mut registry = Registry::with_prefix("shuestand");

        let wallet_sync_total = Counter::<u64>::default();
        registry.register(
            "wallet_sync_total",
            "Number of on-chain wallet sync attempts",
            wallet_sync_total.clone(),
        );

        let wallet_sync_failures = Counter::<u64>::default();
        registry.register(
            "wallet_sync_failure_total",
            "Number of failed on-chain wallet sync attempts",
            wallet_sync_failures.clone(),
        );

        let withdrawal_attempt_total = Counter::<u64>::default();
        registry.register(
            "withdrawal_attempt_total",
            "Number of withdrawal attempts processed by the worker",
            withdrawal_attempt_total.clone(),
        );

        let withdrawal_failure_total = Counter::<u64>::default();
        registry.register(
            "withdrawal_failure_total",
            "Number of withdrawal attempts that failed",
            withdrawal_failure_total.clone(),
        );

        let onchain_float_ratio = Gauge::<i64>::default();
        registry.register(
            "onchain_float_ratio",
            "On-chain wallet balance / target ratio",
            onchain_float_ratio.clone(),
        );

        let cashu_float_ratio = Gauge::<i64>::default();
        registry.register(
            "cashu_float_ratio",
            "Cashu wallet balance / target ratio",
            cashu_float_ratio.clone(),
        );

        let total_float_ratio = Gauge::<i64>::default();
        registry.register(
            "total_float_ratio",
            "Combined on-chain + Cashu balance / target ratio",
            total_float_ratio.clone(),
        );

        let float_drift_sats = Gauge::<i64>::default();
        registry.register(
            "float_drift_sats",
            "Target float - (on-chain + Cashu) balance in sats (negative means surplus)",
            float_drift_sats.clone(),
        );

        Self {
            registry,
            wallet_sync_total,
            wallet_sync_failures,
            withdrawal_attempt_total,
            withdrawal_failure_total,
            onchain_float_ratio,
            cashu_float_ratio,
            total_float_ratio,
            float_drift_sats,
        }
    }

    pub fn inc_wallet_sync(&self) {
        self.wallet_sync_total.inc();
    }

    pub fn inc_wallet_sync_failure(&self) {
        self.wallet_sync_failures.inc();
    }

    pub fn inc_withdrawal_attempt(&self) {
        self.withdrawal_attempt_total.inc();
    }

    pub fn inc_withdrawal_failure(&self) {
        self.withdrawal_failure_total.inc();
    }

    pub fn set_onchain_float_ratio(&self, ratio: f64) {
        self.onchain_float_ratio.set((ratio * 1000.0) as i64);
    }

    pub fn set_cashu_float_ratio(&self, ratio: f64) {
        self.cashu_float_ratio.set((ratio * 1000.0) as i64);
    }

    pub fn set_total_float_ratio(&self, ratio: f64) {
        self.total_float_ratio.set((ratio * 1000.0) as i64);
    }

    pub fn set_float_drift_sats(&self, drift: i64) {
        self.float_drift_sats.set(drift);
    }

    pub fn encode(&self) -> String {
        let mut buffer = String::new();
        encode(&mut buffer, &self.registry).expect("failed to encode metrics");
        buffer
    }
}
