use std::sync::Arc;

use anyhow::anyhow;
use chrono::{DateTime, Utc};
use reqwest::Client;
use serde::Deserialize;
use serde_json::json;
use tokio::sync::RwLock;
use tracing::warn;

use crate::config::FeeEstimatorSettings;

const RPC_ID: &str = "shuestand";

#[derive(Clone, Debug, Default)]
pub struct FeeEstimateEntry {
    pub sats_per_vb: f32,
    pub updated_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug, Default)]
pub struct FeeEstimateSnapshot {
    pub fast: FeeEstimateEntry,
    pub economy: FeeEstimateEntry,
}

pub struct FeeEstimator {
    client: Client,
    rpc_url: Option<String>,
    rpc_auth: Option<(String, String)>,
    fast_target_blocks: u32,
    economy_target_blocks: u32,
    min_sat_per_vb: f32,
    max_sat_per_vb: f32,
    cache: Arc<RwLock<FeeEstimateSnapshot>>,
}

impl FeeEstimator {
    pub fn new(settings: FeeEstimatorSettings) -> Self {
        let mut snapshot = FeeEstimateSnapshot::default();
        snapshot.fast.sats_per_vb = settings.default_fast_sat_per_vb;
        snapshot.economy.sats_per_vb = settings.default_economy_sat_per_vb;

        Self {
            client: Client::new(),
            rpc_url: settings.rpc_url,
            rpc_auth: settings.rpc_user.zip(settings.rpc_password),
            fast_target_blocks: settings.fast_target_blocks.max(1),
            economy_target_blocks: settings.economy_target_blocks.max(1),
            min_sat_per_vb: settings.min_sat_per_vb.max(0.1),
            max_sat_per_vb: settings.max_sat_per_vb.max(settings.min_sat_per_vb),
            cache: Arc::new(RwLock::new(snapshot)),
        }
    }

    pub fn has_remote(&self) -> bool {
        self.rpc_url.is_some()
    }

    pub async fn refresh(&self) -> anyhow::Result<()> {
        let rpc_url = match &self.rpc_url {
            Some(url) => url.clone(),
            None => return Ok(()),
        };

        let fast = match self.fetch_rate(&rpc_url, self.fast_target_blocks).await {
            Ok(rate) => Some(rate),
            Err(err) => {
                warn!(target = "backend", error = %err, "failed to refresh fast fee estimate");
                None
            }
        };

        let economy = match self.fetch_rate(&rpc_url, self.economy_target_blocks).await {
            Ok(rate) => Some(rate),
            Err(err) => {
                warn!(target = "backend", error = %err, "failed to refresh economy fee estimate");
                None
            }
        };

        if fast.is_none() && economy.is_none() {
            return Err(anyhow!("fee estimator refresh failed"));
        }

        let now = Utc::now();
        let mut guard = self.cache.write().await;
        if let Some(rate) = fast {
            guard.fast = FeeEstimateEntry {
                sats_per_vb: rate,
                updated_at: Some(now),
            };
        }
        if let Some(rate) = economy {
            guard.economy = FeeEstimateEntry {
                sats_per_vb: rate,
                updated_at: Some(now),
            };
        }
        Ok(())
    }

    async fn fetch_rate(&self, rpc_url: &str, target_blocks: u32) -> anyhow::Result<f32> {
        let request = json!({
            "jsonrpc": "1.0",
            "id": RPC_ID,
            "method": "estimatesmartfee",
            "params": [target_blocks, "CONSERVATIVE"],
        });

        let mut builder = self.client.post(rpc_url).json(&request);
        if let Some((user, pass)) = &self.rpc_auth {
            builder = builder.basic_auth(user, Some(pass));
        }

        let response = builder.send().await?.error_for_status()?;
        let payload: RpcResponse = response.json().await?;

        if let Some(err) = payload.error {
            return Err(anyhow!("bitcoind error {}: {}", err.code, err.message));
        }

        let result = payload
            .result
            .ok_or_else(|| anyhow!("missing result from estimatesmartfee"))?;

        let feerate_btc_per_kvb = result
            .feerate
            .ok_or_else(|| anyhow!("feerate unavailable"))?;

        // Convert BTC/kvB to sat/vB: (BTC * 1e8) / 1000.
        let sats_per_vb = ((feerate_btc_per_kvb * 1e8f64) / 1000.0) as f32;
        Ok(self.clamp(sats_per_vb))
    }

    fn clamp(&self, value: f32) -> f32 {
        value.max(self.min_sat_per_vb).min(self.max_sat_per_vb)
    }

    pub async fn snapshot(&self) -> FeeEstimateSnapshot {
        self.cache.read().await.clone()
    }

    pub async fn fast_rate(&self) -> f32 {
        self.cache.read().await.fast.sats_per_vb
    }

    pub async fn economy_rate(&self) -> f32 {
        self.cache.read().await.economy.sats_per_vb
    }
}

#[derive(Debug, Deserialize)]
struct RpcResponse {
    result: Option<EstimateSmartFeeResult>,
    error: Option<RpcError>,
}

#[derive(Debug, Deserialize)]
struct EstimateSmartFeeResult {
    feerate: Option<f64>,
}

#[derive(Debug, Deserialize)]
struct RpcError {
    code: i64,
    message: String,
}
