use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::anyhow;
use async_trait::async_trait;
use cdk::wallet::ReceiveOptions;
use tokio::time::sleep;

use crate::cashu::{token_mint_url, token_total_amount};
use crate::db::{Database, Withdrawal, WithdrawalState};
use crate::onchain::OnchainWallet;
use crate::telemetry::AppMetrics;
use crate::wallet::{MintSwapService, MultiMintWalletManager};

pub struct WithdrawalWorker {
    db: Database,
    executor: Arc<dyn WithdrawalExecutor + Send + Sync>,
    interval: Duration,
    max_attempts: u32,
    metrics: Arc<AppMetrics>,
}

impl WithdrawalWorker {
    pub fn new(
        db: Database,
        executor: Arc<dyn WithdrawalExecutor + Send + Sync>,
        interval: Duration,
        max_attempts: u32,
        metrics: Arc<AppMetrics>,
    ) -> Self {
        Self {
            db,
            executor,
            interval,
            max_attempts: max_attempts.max(1),
            metrics,
        }
    }

    pub async fn run(mut self) {
        loop {
            if let Err(err) = self.tick().await {
                tracing::error!(target: "backend", error = %err, "withdrawal worker tick failed");
            }
            sleep(self.interval).await;
        }
    }

    async fn tick(&mut self) -> anyhow::Result<()> {
        let queued = self
            .db
            .list_withdrawals_by_state(&[WithdrawalState::Queued])
            .await?;
        if queued.is_empty() {
            return Ok(());
        }

        tracing::debug!(target: "backend", count = queued.len(), "withdrawal worker processing queued items");
        for withdrawal in queued {
            self.metrics.inc_withdrawal_attempt();
            let started = Instant::now();
            match self.executor.execute(&withdrawal).await {
                Ok(outcome) => {
                    let elapsed_ms = started.elapsed().as_millis() as u64;
                    tracing::info!(
                        target: "backend",
                        id = %withdrawal.id,
                        next_state = outcome.next_state.as_str(),
                        amount_sats = outcome.token_value_sats.unwrap_or_default(),
                        txid = outcome.txid.as_deref().unwrap_or(""),
                        elapsed_ms,
                        "cashu redemption succeeded"
                    );
                    self.db
                        .record_withdrawal_attempt(
                            &withdrawal.id,
                            outcome.next_state,
                            outcome.token_value_sats,
                            outcome.txid.as_deref(),
                            None,
                        )
                        .await?;
                }
                Err(err) => {
                    self.metrics.inc_withdrawal_failure();
                    let attempt_number = withdrawal.attempt_count + 1;
                    let exhausted = attempt_number >= self.max_attempts;
                    let next_state = if exhausted {
                        WithdrawalState::Failed
                    } else {
                        WithdrawalState::Queued
                    };
                    let message = err.to_string();
                    let elapsed_ms = started.elapsed().as_millis() as u64;
                    tracing::warn!(
                        target: "backend",
                        id = %withdrawal.id,
                        attempt = attempt_number,
                        max_attempts = self.max_attempts,
                        exhausted,
                        next_state = next_state.as_str(),
                        error = %message,
                        elapsed_ms,
                        "cashu redemption attempt errored"
                    );
                    self.db
                        .record_withdrawal_attempt(
                            &withdrawal.id,
                            next_state,
                            None,
                            None,
                            Some(message),
                        )
                        .await?;
                }
            }
        }
        Ok(())
    }
}

pub struct WithdrawalOutcome {
    pub next_state: WithdrawalState,
    pub token_value_sats: Option<u64>,
    pub txid: Option<String>,
}

#[async_trait]
pub trait WithdrawalExecutor {
    async fn execute(&self, withdrawal: &Withdrawal) -> Result<WithdrawalOutcome, anyhow::Error>;
}

pub struct CashuRedeemResult {
    pub amount_sats: u64,
    pub swap_fee_sats: Option<u64>,
}

#[async_trait]
pub trait CashuRedeemer: Send + Sync {
    async fn redeem(&self, encoded_token: &str) -> Result<CashuRedeemResult, anyhow::Error>;
}

pub struct CdkCashuRedeemer {
    wallets: Arc<MultiMintWalletManager>,
    swapper: Arc<MintSwapService>,
}

impl CdkCashuRedeemer {
    pub fn new(wallets: Arc<MultiMintWalletManager>, swapper: Arc<MintSwapService>) -> Self {
        Self { wallets, swapper }
    }
}

#[async_trait]
impl CashuRedeemer for CdkCashuRedeemer {
    async fn redeem(&self, encoded_token: &str) -> Result<CashuRedeemResult, anyhow::Error> {
        let mint_url = token_mint_url(encoded_token).map_err(|err| anyhow!(err.to_string()))?;
        let token_amount =
            token_total_amount(encoded_token).map_err(|err| anyhow!(err.to_string()))?;

        if mint_url == self.wallets.canonical_mint() {
            let wallet = self.wallets.wallet_for_mint(&mint_url).await?;
            let amount = wallet
                .lock()
                .await
                .receive(encoded_token, ReceiveOptions::default())
                .await?;
            return Ok(CashuRedeemResult {
                amount_sats: amount.to_u64(),
                swap_fee_sats: None,
            });
        }

        tracing::info!(
            target: "backend",
            %mint_url,
            canonical = self.wallets.canonical_mint(),
            "swapping foreign mint token into canonical float before payout"
        );
        let outcome = self
            .swapper
            .swap_to_canonical(encoded_token, &mint_url, Some(token_amount))
            .await?;
        let fee = outcome
            .original_amount_sats
            .saturating_sub(outcome.canonical_amount_sats);
        Ok(CashuRedeemResult {
            amount_sats: outcome.canonical_amount_sats,
            swap_fee_sats: Some(fee),
        })
    }
}

pub struct CashuRedeemingExecutor {
    pub redeemer: Arc<dyn CashuRedeemer>,
    pub db: Database,
}

#[async_trait]
impl WithdrawalExecutor for CashuRedeemingExecutor {
    async fn execute(&self, withdrawal: &Withdrawal) -> Result<WithdrawalOutcome, anyhow::Error> {
        let amount_sats = if withdrawal.token_consumed {
            withdrawal
                .token_value_sats
                .ok_or_else(|| anyhow!("withdrawal marked consumed without amount"))?
        } else {
            let token = withdrawal
                .token
                .as_deref()
                .ok_or_else(|| anyhow!("withdrawal missing token payload"))?;
            let redeemed = self.redeemer.redeem(token).await?;
            self.db
                .record_token_consumed(&withdrawal.id, redeemed.amount_sats, redeemed.swap_fee_sats)
                .await?;
            redeemed.amount_sats
        };

        Ok(WithdrawalOutcome {
            next_state: WithdrawalState::Broadcasting,
            token_value_sats: Some(amount_sats),
            txid: None,
        })
    }
}

pub struct CashuToOnchainExecutor {
    redeemer: Arc<dyn CashuRedeemer>,
    wallet: Arc<OnchainWallet>,
    payout_fee_rate_vb: f32,
    db: Database,
}

impl CashuToOnchainExecutor {
    pub fn new(
        redeemer: Arc<dyn CashuRedeemer>,
        wallet: Arc<OnchainWallet>,
        payout_fee_rate_vb: f32,
        db: Database,
    ) -> Self {
        Self {
            redeemer,
            wallet,
            payout_fee_rate_vb: payout_fee_rate_vb.max(0.1),
            db,
        }
    }
}

#[async_trait]
impl WithdrawalExecutor for CashuToOnchainExecutor {
    async fn execute(&self, withdrawal: &Withdrawal) -> Result<WithdrawalOutcome, anyhow::Error> {
        let amount_sats = if withdrawal.token_consumed {
            withdrawal
                .token_value_sats
                .ok_or_else(|| anyhow!("withdrawal marked consumed without amount"))?
        } else {
            let token = withdrawal
                .token
                .as_deref()
                .ok_or_else(|| anyhow!("withdrawal missing token payload"))?;
            let redeemed = self.redeemer.redeem(token).await?;
            self.db
                .record_token_consumed(&withdrawal.id, redeemed.amount_sats, redeemed.swap_fee_sats)
                .await?;
            redeemed.amount_sats
        };

        let txid = self
            .wallet
            .send_to_address(
                &withdrawal.delivery_address,
                amount_sats,
                self.payout_fee_rate_vb,
            )
            .await?;

        Ok(WithdrawalOutcome {
            next_state: WithdrawalState::Confirming,
            token_value_sats: Some(amount_sats),
            txid: Some(txid),
        })
    }
}

pub struct MockWithdrawalExecutor;

#[async_trait]
impl WithdrawalExecutor for MockWithdrawalExecutor {
    async fn execute(&self, withdrawal: &Withdrawal) -> Result<WithdrawalOutcome, anyhow::Error> {
        let inferred_amount = withdrawal.token_value_sats.unwrap_or_else(|| {
            withdrawal
                .token
                .as_ref()
                .map(|token| (token.len() as u64).max(100))
                .unwrap_or(100)
        });
        Ok(WithdrawalOutcome {
            next_state: WithdrawalState::Confirming,
            token_value_sats: Some(inferred_amount),
            txid: Some(format!("mock-tx-{}", withdrawal.id)),
        })
    }
}
