use crate::db::Database;
use chrono::Utc;
use reqwest::Client;
use serde_json::json;
use tracing::{info, warn};

#[derive(Clone)]
pub struct TransactionNotifier {
    db: Database,
    client: Client,
    webhook_url: String,
}

impl TransactionNotifier {
    pub fn new(db: Database, webhook_url: String) -> Self {
        Self {
            db,
            client: Client::new(),
            webhook_url,
        }
    }

    pub fn maybe_new(db: Database, webhook_url: Option<String>) -> Option<Self> {
        webhook_url.map(|url| Self::new(db, url))
    }

    pub async fn record_deposit(&self, deposit_id: &str) {
        self.record(TransactionKind::DepositFulfilled, deposit_id)
            .await;
    }

    pub async fn record_withdrawal(&self, withdrawal_id: &str) {
        self.record(TransactionKind::WithdrawalSettled, withdrawal_id)
            .await;
    }

    async fn record(&self, kind: TransactionKind, entity_id: &str) {
        let marked = match kind {
            TransactionKind::DepositFulfilled => {
                self.db.mark_deposit_transaction_counted(entity_id).await
            }
            TransactionKind::WithdrawalSettled => {
                self.db.mark_withdrawal_transaction_counted(entity_id).await
            }
        };

        let marked = match marked {
            Ok(flag) => flag,
            Err(err) => {
                warn!(
                    target = "backend",
                    error = %err,
                    entity_id,
                    kind = kind.as_str(),
                    "failed to update transaction_counted_at"
                );
                return;
            }
        };

        if !marked {
            return;
        }

        let counter = match self.db.increment_transaction_counter().await {
            Ok(value) => value,
            Err(err) => {
                warn!(
                    target = "backend",
                    error = %err,
                    entity_id,
                    kind = kind.as_str(),
                    "failed to increment transaction counter"
                );
                return;
            }
        };

        let payload = json!({
            "event": "transaction_counter",
            "counter": counter,
            "kind": kind.as_str(),
            "entity_id": entity_id,
            "timestamp": Utc::now().to_rfc3339(),
        });

        match self
            .client
            .post(&self.webhook_url)
            .json(&payload)
            .send()
            .await
        {
            Ok(resp) if resp.status().is_success() => {
                info!(
                    target = "backend",
                    counter,
                    kind = kind.as_str(),
                    entity_id,
                    "transaction counter webhook delivered"
                );
            }
            Ok(resp) => {
                warn!(
                    target = "backend",
                    status = resp.status().as_u16(),
                    counter,
                    kind = kind.as_str(),
                    entity_id,
                    "transaction counter webhook returned non-success"
                );
            }
            Err(err) => {
                warn!(
                    target = "backend",
                    error = %err,
                    counter,
                    kind = kind.as_str(),
                    entity_id,
                    "failed to POST transaction counter webhook"
                );
            }
        }
    }
}

#[derive(Clone, Copy)]
enum TransactionKind {
    DepositFulfilled,
    WithdrawalSettled,
}

impl TransactionKind {
    fn as_str(&self) -> &'static str {
        match self {
            TransactionKind::DepositFulfilled => "deposit",
            TransactionKind::WithdrawalSettled => "withdrawal",
        }
    }
}
