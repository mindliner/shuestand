use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, anyhow};
use cdk::Amount;
use cdk::amount::SplitTarget;
use cdk::nuts::nut00::KnownMethod;
use cdk::nuts::Token;
use cdk::nuts::{PaymentMethod, ProofsMethods, State};
use cdk::wallet::{KeysetFilter, MeltQuote, ReceiveOptions};
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
            .receive_foreign_token(
                &foreign_wallet,
                foreign_mint_url,
                token_raw,
                expected_amount,
            )
            .await?;
        if original_amount == 0 {
            return Err(anyhow!("foreign token had zero value"));
        }

        let (mut proofs_total, mut input_fee_total) = self
            .spendable_amount_and_fee(&foreign_wallet)
            .await
            .context("reading foreign wallet balance for swap")?;
        if proofs_total == 0 || proofs_total <= input_fee_total {
            return Err(anyhow!(
                "foreign wallet has no spendable balance after importing token"
            ));
        }

        let canonical_wallet = self.manager.canonical_wallet();
        let mut target_amount = proofs_total
            .saturating_sub(input_fee_total)
            .min(original_amount);
        let mut last_err: Option<anyhow::Error> = None;

        for _ in 0..=MAX_MELT_FEE_REDUCTIONS {
            if target_amount == 0 {
                break;
            }

            let quote = {
                let guard = canonical_wallet.lock().await;
                guard
                    .mint_quote(
                        PaymentMethod::Known(KnownMethod::Bolt11),
                        Some(Amount::from(target_amount)),
                        None,
                        None,
                    )
                    .await
                    .context("requesting mint quote from canonical mint")?
            };

            let melt_quote = {
                let guard = foreign_wallet.lock().await;
                guard
                    .melt_quote(
                        PaymentMethod::Known(KnownMethod::Bolt11),
                        &quote.request,
                        None,
                        None,
                    )
                    .await
                    .context("requesting melt quote from foreign mint")?
            };
            let fee_reserve = melt_quote.fee_reserve.to_u64();

            let needed = target_amount
                .saturating_add(fee_reserve)
                .saturating_add(input_fee_total);
            if needed > proofs_total {
                let mut next_target = proofs_total
                    .saturating_sub(input_fee_total + fee_reserve)
                    .min(target_amount);
                if next_target >= target_amount {
                    next_target = target_amount.saturating_sub(fee_reserve.max(1));
                }
                if next_target == 0 {
                    last_err = Some(anyhow!(
                        "insufficient melt balance (fee_reserve={fee_reserve})"
                    ));
                    break;
                }
                tracing::warn!(
                    target: "backend",
                    target_amount,
                    fee_reserve,
                    spendable_sats = proofs_total,
                    input_fee_sats = input_fee_total,
                    next_target,
                    "melt would exceed spendable balance; shrinking invoice before attempting payment"
                );
                target_amount = next_target;
                continue;
            }

            match self
                .confirm_melt_with_quote(&foreign_wallet, melt_quote)
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
                    last_err = Some(anyhow!(
                        "insufficient melt balance (fee_reserve={fee_reserve})"
                    ));
                    let (updated_total, updated_fee) = self
                        .spendable_amount_and_fee(&foreign_wallet)
                        .await
                        .context("refreshing foreign wallet balance after melt failure")?;
                    proofs_total = updated_total;
                    input_fee_total = updated_fee;
                    if proofs_total == 0 || proofs_total <= input_fee_total {
                        break;
                    }
                    let mut next_target = proofs_total
                        .saturating_sub(input_fee_total + fee_reserve)
                        .min(target_amount);
                    if next_target >= target_amount {
                        next_target = target_amount.saturating_sub(fee_reserve.max(1));
                    }
                    if next_target == 0 {
                        break;
                    }
                    tracing::warn!(
                        target: "backend",
                        target_amount,
                        fee_reserve,
                        spendable_sats = proofs_total,
                        input_fee_sats = input_fee_total,
                        next_target,
                        "foreign melt still reported insufficient funds after swap; shrinking invoice again"
                    );
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
        mint_url: &str,
        token_raw: &str,
        expected_amount: Option<u64>,
    ) -> anyhow::Result<u64> {
        let receive_result = receive_with_all_keysets(wallet, token_raw).await;

        match receive_result {
            Ok(amount) => Ok(amount.to_u64()),
            Err(err) => {
                let message = err.to_string();
                if message.contains("Invalid DLEQ proof") {
                    tracing::warn!(
                        target: "backend",
                        mint_url = %mint_url,
                        token_chars = token_raw.len(),
                        expected_amount_sats = expected_amount.unwrap_or(0),
                        "foreign mint rejected token due to invalid DLEQ proof"
                    );
                }
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

    async fn confirm_melt_with_quote(
        &self,
        wallet: &WalletHandle,
        melt_quote: MeltQuote,
    ) -> Result<(), MeltPaymentError> {
        let guard = wallet.lock().await;
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
                if matches!(
                    status.state,
                    cdk::nuts::MintQuoteState::Issued | cdk::nuts::MintQuoteState::Paid
                ) {
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

    async fn spendable_amount_and_fee(&self, wallet: &WalletHandle) -> anyhow::Result<(u64, u64)> {
        let guard = wallet.lock().await;
        let proofs = guard
            .get_proofs_with(Some(vec![State::Unspent]), None)
            .await
            .context("fetching unspent proofs for swap")?;
        let total = proofs
            .total_amount()
            .context("summing foreign proof amounts")?
            .to_u64();
        let fee = guard
            .get_proofs_fee(&proofs)
            .await
            .context("estimating input fee for foreign proofs")?
            .total
            .to_u64();
        Ok((total, fee))
    }
}

async fn receive_with_all_keysets(
    wallet: &WalletHandle,
    token_raw: &str,
) -> Result<cdk::Amount, cdk::Error> {
    let token = Token::from_str(token_raw)?;
    let guard = wallet.lock().await;
    let keysets = guard.get_mint_keysets(KeysetFilter::All).await?;
    let proofs = token.proofs(&keysets)?;
    guard
        .receive_proofs(
            proofs,
            ReceiveOptions::default(),
            token.memo().clone(),
            Some(token_raw.to_string()),
        )
        .await
}
