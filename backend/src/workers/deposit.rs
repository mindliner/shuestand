use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, anyhow};
use async_trait::async_trait;
use cdk::Amount;
use cdk::wallet::{SendOptions, Wallet as CashuWallet};
use reqwest::{Client, Url};
use serde::Serialize;
use tokio::sync::Mutex;
use tokio::time::sleep;

use crate::db::{Database, DepositState};

pub struct DepositWorker {
    db: Database,
    sender: Arc<dyn DepositTokenSender + Send + Sync>,
    interval: Duration,
    max_attempts: u32,
    http: Client,
}

impl DepositWorker {
    pub fn new(
        db: Database,
        sender: Arc<dyn DepositTokenSender + Send + Sync>,
        interval: Duration,
        max_attempts: u32,
        http: Client,
    ) -> Self {
        Self {
            db,
            sender,
            interval,
            max_attempts: max_attempts.max(1),
            http,
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

            let minted_token = match deposit.minted_token.as_deref() {
                Some(token) => token,
                None => {
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
            };

            if let Some(hint) = deposit.delivery_hint.as_deref() {
                match parse_delivery_hint(hint) {
                    DeliveryHintKind::Webhook(url) => {
                        tracing::info!(
                            target: "backend",
                            id = %deposit.id,
                            url = %url,
                            "attempting webhook delivery"
                        );
                        match self
                            .deliver_webhook(
                                &url,
                                &deposit.id,
                                deposit.amount_sats,
                                minted_token,
                                deposit.delivery_hint.as_deref(),
                                deposit.txid.as_deref(),
                            )
                            .await
                        {
                            Ok(_) => {
                                tracing::info!(
                                    target: "backend",
                                    id = %deposit.id,
                                    "webhook delivery succeeded"
                                );
                                self.db
                                    .record_delivery_success(&deposit.id, DepositState::Ready)
                                    .await?;
                                self.db.record_pickup_success(&deposit.id).await?;
                                continue;
                            }
                            Err(err) => {
                                let message = err.to_string();
                                tracing::warn!(
                                    target: "backend",
                                    id = %deposit.id,
                                    attempt = attempt_number,
                                    max_attempts = self.max_attempts,
                                    error = %message,
                                    "webhook delivery failed"
                                );
                                self.db
                                    .record_delivery_failure(
                                        &deposit.id,
                                        DepositState::Ready,
                                        &message,
                                    )
                                    .await?;
                                continue;
                            }
                        }
                    }
                    DeliveryHintKind::Unsupported => {
                        tracing::debug!(
                            target: "backend",
                            id = %deposit.id,
                            hint,
                            "delivery hint unsupported for auto delivery"
                        );
                    }
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

    async fn deliver_webhook(
        &self,
        url: &Url,
        deposit_id: &str,
        amount_sats: u64,
        token: &str,
        hint: Option<&str>,
        txid: Option<&str>,
    ) -> anyhow::Result<()> {
        let payload = DeliveryWebhookPayload {
            deposit_id,
            amount_sats,
            token,
            txid,
            hint,
        };
        let resp = self
            .http
            .post(url.clone())
            .json(&payload)
            .send()
            .await
            .with_context(|| format!("failed to POST webhook for {}", deposit_id))?;
        if resp.status().is_success() {
            Ok(())
        } else {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            Err(anyhow!("webhook {} returned {} {}", url, status, body))
        }
    }
}

enum DeliveryHintKind {
    Webhook(Url),
    Unsupported,
}

fn parse_delivery_hint(raw: &str) -> DeliveryHintKind {
    if raw.starts_with("http://") || raw.starts_with("https://") {
        if let Ok(url) = Url::parse(raw) {
            return DeliveryHintKind::Webhook(url);
        }
    }
    DeliveryHintKind::Unsupported
}

#[derive(Serialize)]
struct DeliveryWebhookPayload<'a> {
    deposit_id: &'a str,
    amount_sats: u64,
    token: &'a str,
    txid: Option<&'a str>,
    hint: Option<&'a str>,
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
