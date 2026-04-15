use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use cdk::Amount;
use cdk::wallet::{SendOptions, Wallet as CashuWallet};
use reqwest::Client;
use tokio::sync::{Mutex, RwLock};
use tokio::time::sleep;

use crate::db::{Database, DepositState};
use crate::operations::OperationMode;
use crate::transactions::TransactionNotifier;

pub struct DepositWorker {
    db: Database,
    sender: Arc<dyn DepositTokenSender + Send + Sync>,
    interval: Duration,
    max_attempts: u32,
    operation_mode: Arc<RwLock<OperationMode>>,
}

impl DepositWorker {
    pub fn new(
        db: Database,
        sender: Arc<dyn DepositTokenSender + Send + Sync>,
        interval: Duration,
        max_attempts: u32,
        _http: Client,
        operation_mode: Arc<RwLock<OperationMode>>,
        _transaction_notifier: Option<Arc<TransactionNotifier>>,
    ) -> Self {
        Self {
            db,
            sender,
            interval,
            max_attempts: max_attempts.max(1),
            operation_mode,
        }
    }

    pub async fn run(mut self) {
        loop {
            if self.should_pause().await {
                sleep(self.interval).await;
                continue;
            }
            if let Err(err) = self.tick().await {
                tracing::error!(target: "backend", error = %err, "deposit worker tick failed");
            }
            sleep(self.interval).await;
        }
    }

    async fn should_pause(&self) -> bool {
        matches!(*self.operation_mode.read().await, OperationMode::Halt)
    }

    async fn tick(&mut self) -> anyhow::Result<()> {
        let minting = self
            .db
            .list_deposits_by_state(&[DepositState::Minting])
            .await?;
        for deposit in minting {
            let attempt_number = deposit.mint_attempt_count + 1;
            match self.sender.mint_token(deposit.amount_sats).await {
                Ok(minted) => {
                    tracing::info!(
                        target: "backend",
                        id = %deposit.id,
                        amount_sats = minted.amount_sats,
                        "deposit minted cashu token"
                    );
                    self.db
                        .record_mint_success(
                            &deposit.id,
                            &minted.token,
                            minted.amount_sats,
                            DepositState::Delivering,
                        )
                        .await?;
                }
                Err(err) => {
                    let exhausted = attempt_number >= self.max_attempts;
                    let next_state = if exhausted {
                        DepositState::Failed
                    } else {
                        DepositState::Minting
                    };
                    let message = err.to_string();
                    tracing::warn!(
                        target: "backend",
                        id = %deposit.id,
                        attempt = attempt_number,
                        max_attempts = self.max_attempts,
                        exhausted,
                        error = %message,
                        "deposit mint attempt failed"
                    );
                    self.db
                        .record_mint_failure(&deposit.id, next_state, &message)
                        .await?;
                }
            }
        }

        let delivering = self
            .db
            .list_deposits_by_state(&[DepositState::Delivering])
            .await?;
        for deposit in delivering {
            let attempt_number = deposit.delivery_attempt_count + 1;
            let exhausted = attempt_number >= self.max_attempts;

            if deposit.minted_token.as_deref().is_none() {
                self.db
                    .record_delivery_failure(
                        &deposit.id,
                        if exhausted {
                            DepositState::Failed
                        } else {
                            DepositState::Minting
                        },
                        "missing minted token; returning to minting",
                    )
                    .await?;
                continue;
            }

            if let Some(hint) = deposit.delivery_hint.as_deref() {
                if parse_delivery_hint(hint) == DeliveryHintKind::Unsupported {
                    tracing::debug!(
                        target: "backend",
                        id = %deposit.id,
                        hint,
                        "delivery hint unsupported for auto delivery"
                    );
                }
            }

            tracing::info!(
                target: "backend",
                id = %deposit.id,
                "deposit token ready for pickup"
            );
            self.db
                .record_delivery_success(&deposit.id, DepositState::Ready)
                .await?;
        }

        Ok(())
    }

}

#[derive(PartialEq, Eq)]
enum DeliveryHintKind {
    Unsupported,
}

fn parse_delivery_hint(_raw: &str) -> DeliveryHintKind {
    // User-supplied webhook delivery hints are intentionally disabled.
    DeliveryHintKind::Unsupported
}

pub struct MintedToken {
    pub amount_sats: u64,
    pub token: String,
}

#[async_trait]
pub trait DepositTokenSender {
    async fn mint_token(&self, amount_sats: u64) -> Result<MintedToken, anyhow::Error>;
}

pub struct CashuTokenSender {
    wallet: Arc<Mutex<CashuWallet>>,
}

impl CashuTokenSender {
    pub fn new(wallet: Arc<Mutex<CashuWallet>>) -> Self {
        Self { wallet }
    }
}

#[async_trait]
impl DepositTokenSender for CashuTokenSender {
    async fn mint_token(&self, amount_sats: u64) -> Result<MintedToken, anyhow::Error> {
        let wallet = self.wallet.lock().await;
        let prepared = wallet
            .prepare_send(Amount::from(amount_sats), SendOptions::default())
            .await?;
        let token = prepared.confirm(None).await?;
        Ok(MintedToken {
            amount_sats,
            token: token.to_v3_string(),
        })
    }
}
