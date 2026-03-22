use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, anyhow};
use cdk::Amount;
use cdk::amount::SplitTarget;
use cdk::nuts::PaymentMethod;
use cdk::nuts::nut00::KnownMethod;
use cdk::wallet::ReceiveOptions;
use thiserror::Error;
use tokio::time::sleep;

use super::{MultiMintWalletManager, WalletHandle};

const DEFAULT_SWAP_POLL_INTERVAL: Duration = Duration::from_secs(2);
const DEFAULT_SWAP_MAX_ATTEMPTS: u32 = 30;
const MAX_MELT_FEE_REDUCTIONS: u32 = 32;

#[derive(Clone, Debug)]
pub struct MintSwapOutcome {
    pub original_amount_sats: u64,
    pub canonical_amount_sats: u64,
}

#[derive(Error, Debug)]
pub enum MeltPaymentError {
    #[error("insufficient funds to pay melt quote (fee_reserve={fee_reserve})")]
    InsufficientFunds { fee_reserve: u64 },
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

pub struct MintSwapService {
    manager: Arc<MultiMintWalletManager>,
    poll_interval: Duration,
    max_attempts: u32,
}

impl MintSwapService {
    pub fn new(manager: Arc<MultiMintWalletManager>) -> Self {
        Self {
            manager,
            poll_interval: DEFAULT_SWAP_POLL_INTERVAL,
            max_attempts: DEFAULT_SWAP_MAX_ATTEMPTS,
        }
    }

    pub async fn swap_to_canonical(
        &self,
        token_raw: &str,
        foreign_mint_url: &str,
        expected_amount: Option<u64>,
    ) -> anyhow::Result<MintSwapOutcome> {
        let foreign_wallet = self.manager.wallet_for_mint(foreign_mint_url).await?;
        let original_amount = self
            .receive_foreign_token(&foreign_wallet, token_raw, expected_amount)
            .await?;
        if original_amount == 0 {
            return Err(anyhow!("foreign token had zero value"));
        }

        let canonical_wallet = self.manager.canonical_wallet();
        let mut target_amount = original_amount;
        let mut last_err: Option<anyhow::Error> = None;

        for _ in 0..=MAX_MELT_FEE_REDUCTIONS {
            let quote = {
                let guard = canonical_wallet.lock().await;
                guard
                    .mint_quote(
                        KnownMethod::Bolt11,
                        Some(Amount::from(target_amount)),
                        None,
                        None,
                    )
                    .await
                    .context("requesting mint quote from canonical mint")?
            };

            match self
                .pay_invoice_with_foreign_wallet(&foreign_wallet, &quote.request)
                .await
            {
                Ok(_) => {
                    let canonical_amount = self
                        .mint_canonical_proofs(&canonical_wallet, &quote.id)
                        .await?;
                    return Ok(MintSwapOutcome {
                        original_amount_sats: original_amount,
                        canonical_amount_sats: canonical_amount,
                    });
                }
                Err(MeltPaymentError::InsufficientFunds { fee_reserve }) => {
                    if target_amount == 0 {
                        return Err(anyhow!(
                            "foreign melt reported insufficient funds even after exhausting invoice reductions"
                        ));
                    }
                    let fee_ratio = if target_amount > 0 {
                        fee_reserve as f64 / target_amount as f64
                    } else {
                        0.0
                    };
                    let mut next_target = if fee_ratio > 0.0 {
                        ((original_amount as f64) / (1.0 + fee_ratio)).floor() as u64
                    } else {
                        original_amount.saturating_sub(fee_reserve)
                    };
                    if next_target >= target_amount {
                        next_target = target_amount.saturating_sub(fee_reserve.max(1));
                    }
                    tracing::warn!(
                        target: "backend",
                        target_amount,
                        fee_reserve,
                        fee_ratio,
                        next_target,
                        "foreign melt reported insufficient funds; recalculating invoice amount"
                    );
                    last_err = Some(anyhow!(
                        "insufficient melt balance (fee_reserve={fee_reserve})"
                    ));
                    target_amount = next_target;
                    continue;
                }
                Err(MeltPaymentError::Other(err)) => {
                    return Err(err);
                }
            }
        }

        Err(last_err.unwrap_or_else(|| {
            anyhow!("foreign melt never succeeded after reducing canonical invoice amount")
        }))
    }

    async fn receive_foreign_token(
        &self,
        wallet: &WalletHandle,
        token_raw: &str,
        expected_amount: Option<u64>,
    ) -> anyhow::Result<u64> {
        let receive_result = wallet
            .lock()
            .await
            .receive(token_raw, ReceiveOptions::default())
            .await;

        match receive_result {
            Ok(amount) => Ok(amount.to_u64()),
            Err(err) => {
                let message = err.to_string();
                if let Some(expected) = expected_amount {
                    if message.contains("Token Already Spent") {
                        tracing::warn!(
                            target: "backend",
                            expected_amount = expected,
                            "foreign token already imported; reusing existing proofs"
                        );
                        return Ok(expected);
                    }
                }
                Err(err.into())
            }
        }
    }

    async fn pay_invoice_with_foreign_wallet(
        &self,
        wallet: &WalletHandle,
        invoice: &str,
    ) -> Result<(), MeltPaymentError> {
        let guard = wallet.lock().await;
        let melt_quote = guard
            .melt_quote(
                PaymentMethod::Known(KnownMethod::Bolt11),
                invoice,
                None,
                None,
            )
            .await
            .map_err(|err| MeltPaymentError::Other(err.into()))?;

        let prepared = guard
            .prepare_melt(&melt_quote.id, HashMap::new())
            .await
            .map_err(|err| {
                let message = err.to_string();
                if message.contains("Insufficient funds") {
                    let fee_reserve = melt_quote.fee_reserve.to_u64();
                    MeltPaymentError::InsufficientFunds { fee_reserve }
                } else {
                    MeltPaymentError::Other(anyhow!("preparing melt: {err}"))
                }
            })?;

        prepared.confirm().await.map_err(|err| {
            MeltPaymentError::Other(anyhow!("confirming melt/payment from foreign mint: {err}"))
        })?;
        Ok(())
    }

    async fn mint_canonical_proofs(
        &self,
        wallet: &WalletHandle,
        quote_id: &str,
    ) -> anyhow::Result<u64> {
        for _ in 0..self.max_attempts {
            {
                let guard = wallet.lock().await;
                let status = guard
                    .check_mint_quote_status(quote_id)
                    .await
                    .context("checking mint quote status")?;
                if status.state == cdk::nuts::MintQuoteState::Issued {
                    drop(guard);
                    let guard = wallet.lock().await;
                    let proofs = guard
                        .mint(quote_id, SplitTarget::default(), None)
                        .await
                        .context("minting canonical proofs")?;
                    let amount: u64 = proofs.iter().map(|p| p.amount.clone().to_u64()).sum();
                    if amount == 0 {
                        return Err(anyhow!("canonical mint returned zero-value proofs"));
                    }
                    return Ok(amount);
                }
            }
            sleep(self.poll_interval).await;
        }
        Err(anyhow!(
            "mint quote {} was never issued after polling",
            quote_id
        ))
    }
}
