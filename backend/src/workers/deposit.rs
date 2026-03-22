use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use cdk::Amount;
use cdk::wallet::{SendOptions, Wallet as CashuWallet};
use tokio::sync::Mutex;
use tokio::time::sleep;

use crate::db::{Database, DepositState};

pub struct DepositWorker {
    db: Database,
    sender: Arc<dyn DepositTokenSender + Send + Sync>,
    interval: Duration,
    max_attempts: u32,
}

impl DepositWorker {
    pub fn new(
        db: Database,
        sender: Arc<dyn DepositTokenSender + Send + Sync>,
        interval: Duration,
        max_attempts: u32,
    ) -> Self {
        Self {
            db,
            sender,
            interval,
            max_attempts: max_attempts.max(1),
        }
    }

    pub async fn run(mut self) {
        loop {
            if let Err(err) = self.tick().await {
                tracing::error!(target: "backend", error = %err, "deposit worker tick failed");
            }
            sleep(self.interval).await;
        }
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

            if deposit.minted_token.is_none() {
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
            token: token.to_string(),
        })
    }
}
