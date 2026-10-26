use std::collections::{HashMap, HashSet};
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, anyhow};
use cdk::Amount;
use cdk::amount::SplitTarget;
use cdk::nuts::Token;
use cdk::nuts::nut00::KnownMethod;
use cdk::nuts::{
    MeltQuoteState, PaymentMethod, ProofState, Proofs, ProofsMethods, PublicKey, State,
};
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
        // Never infer ownership from a spent-token error or a pooled wallet balance.
        let original_amount =
            crate::cashu::token_total_amount(token_raw).map_err(|err| anyhow!(err.to_string()))?;
        if expected_amount.is_some_and(|expected| expected != original_amount) {
            return Err(anyhow!(
                "foreign token amount does not match expected amount"
            ));
        }
        let (received_amount, proofs) = self
            .receive_foreign_token(&foreign_wallet, token_raw)
            .await?;
        validate_conversion_amount(original_amount, received_amount)?;
        let (proofs_total, input_fee_total) = self
            .spendable_amount_and_fee(&foreign_wallet, &proofs)
            .await
            .context("checking transaction-specific foreign proofs")?;

        let canonical_wallet = self.manager.canonical_wallet();
        let mut target_amount = proofs_total
            .saturating_sub(input_fee_total)
            .min(original_amount);
        let mut last_err: Option<anyhow::Error> = None;

        for _ in 0..=MAX_MELT_FEE_REDUCTIONS {
            validate_conversion_amount(original_amount, target_amount)?;

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
                    .saturating_sub(input_fee_total.saturating_add(fee_reserve))
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
                .confirm_melt_with_quote(&foreign_wallet, melt_quote, &proofs)
                .await
            {
                Ok(_) => {
                    let canonical_amount = self
                        .mint_canonical_proofs(&canonical_wallet, &quote.id)
                        .await?;
                    if canonical_amount != target_amount {
                        return Err(anyhow!(
                            "canonical mint amount differs from paid invoice; reconciliation required"
                        ));
                    }
                    return Ok(MintSwapOutcome {
                        original_amount_sats: original_amount,
                        canonical_amount_sats: canonical_amount,
                    });
                }
                Err(MeltPaymentError::InsufficientFunds { fee_reserve }) => {
                    // No balance-based retry after a prepared operation: its proofs may
                    // be reserved or payment may be ambiguous. Keep evidence for recovery.
                    return Err(anyhow!(
                        "insufficient melt balance (fee_reserve={fee_reserve}); reconciliation required"
                    ));
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
    ) -> anyhow::Result<(u64, Proofs)> {
        let token = Token::from_str(token_raw)?;
        // Keep the snapshot, receive and output capture under one wallet lock.
        let guard = wallet.lock().await;
        let before = guard.get_proofs_with(None, None).await?.ys()?;
        let keysets = guard.get_mint_keysets(KeysetFilter::All).await?;
        let inputs = token.proofs(&keysets)?;
        let received = guard.receive_proofs(
            inputs, ReceiveOptions::default(), token.memo().clone(),
            Some(token_raw.to_string()),
        ).await.context("receiving foreign token; do not reuse pooled proofs on failure, reconcile this operation before retrying")?;
        let after = guard.get_unspent_proofs().await?;
        let proofs = received_proofs(&before, after, received.to_u64())?;
        Ok((received.to_u64(), proofs))
    }

    async fn confirm_melt_with_quote(
        &self,
        wallet: &WalletHandle,
        melt_quote: MeltQuote,
        proofs: &Proofs,
    ) -> Result<(), MeltPaymentError> {
        let guard = wallet.lock().await;
        let states = guard
            .check_proofs_spent(proofs.clone())
            .await
            .map_err(|err| MeltPaymentError::Other(anyhow!("checking melt proof states: {err}")))?;
        require_unspent(proofs, &states).map_err(MeltPaymentError::Other)?;
        let prepared = guard
            .prepare_melt_proofs(&melt_quote.id, proofs.clone(), HashMap::new())
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

        let finalized = prepared.confirm().await.map_err(|err| {
            MeltPaymentError::Other(anyhow!("confirming melt/payment from foreign mint: {err}"))
        })?;
        if finalized.state() != MeltQuoteState::Paid {
            return Err(MeltPaymentError::Other(anyhow!(
                "foreign melt not paid; reconciliation required"
            )));
        }
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

    async fn spendable_amount_and_fee(
        &self,
        wallet: &WalletHandle,
        proofs: &Proofs,
    ) -> anyhow::Result<(u64, u64)> {
        let guard = wallet.lock().await;
        let states = guard.check_proofs_spent(proofs.clone()).await?;
        require_unspent(proofs, &states)?;
        let total = proofs
            .total_amount()
            .context("summing foreign proof amounts")?
            .to_u64();
        let fee = guard
            .get_proofs_fee(proofs)
            .await
            .context("estimating input fee for foreign proofs")?
            .total
            .to_u64();
        Ok((total, fee))
    }
}

// Safety ceiling, not a quoted or charged fee. Refuse excessive loss before paying.
// Five percent accommodates the observed ~3% reserve without allowing dust fallback.
pub(crate) fn validate_conversion_amount(original: u64, converted: u64) -> anyhow::Result<()> {
    if original == 0
        || converted == 0
        || converted > original
        || original - converted > original / 20
    {
        return Err(anyhow!(
            "conversion exceeds 5% loss limit or has invalid amount; reconciliation required"
        ));
    }
    Ok(())
}

fn received_proofs(before: &[PublicKey], after: Proofs, received: u64) -> anyhow::Result<Proofs> {
    let existing: HashSet<_> = before.iter().copied().collect();
    let mut fresh = Vec::new();
    for proof in after {
        if !existing.contains(&proof.y()?) {
            fresh.push(proof);
        }
    }
    if fresh.is_empty() || fresh.total_amount()?.to_u64() != received {
        return Err(anyhow!(
            "received proofs do not match receive amount; reconciliation required"
        ));
    }
    Ok(fresh)
}

fn require_unspent(proofs: &Proofs, states: &[ProofState]) -> anyhow::Result<()> {
    let expected: HashSet<_> = proofs.ys()?.into_iter().collect();
    let returned: HashSet<_> = states.iter().map(|state| state.y).collect();
    if expected.is_empty()
        || expected.len() != proofs.len()
        || returned != expected
        || states.len() != expected.len()
        || states.iter().any(|state| state.state != State::Unspent)
    {
        return Err(anyhow!(
            "proofs are spent, pending or have incomplete mint state; reconciliation required"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use cdk::nuts::{Id, Proof};

    fn proof(secret: &str, amount: u64) -> Proof {
        Proof::new(
            Amount::from(amount),
            Id::from_str("008aa8c3ea7e4220").unwrap(),
            cdk::secret::Secret::new(secret),
            PublicKey::from_str(
                "0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798",
            )
            .unwrap(),
        )
    }

    #[test]
    fn isolates_receive_outputs_from_old_wallet_proofs() {
        let stale = proof("old July proof", 4096);
        let unrelated = proof("another customer's proof", 32768);
        let fresh = proof("this customer's replacement", 114998);
        let before = vec![stale.clone(), unrelated.clone()].ys().unwrap();
        let selected =
            received_proofs(&before, vec![stale, fresh.clone(), unrelated], 114998).unwrap();
        assert_eq!(selected, vec![fresh]);
    }

    #[test]
    fn missing_receive_outputs_cannot_fall_back_to_wallet_dust() {
        let dust = proof("unrelated dust", 18);
        let before = vec![dust.clone()].ys().unwrap();
        assert!(received_proofs(&before, vec![dust], 114998).is_err());
        assert!(received_proofs(&[], vec![proof("partial receive", 13)], 114998).is_err());
    }

    #[test]
    fn rejects_spent_pending_missing_duplicate_and_unexpected_mint_states() {
        let proofs = vec![proof("fresh", 1000)];
        let y = proofs.ys().unwrap()[0];
        let unspent = ProofState::from((y, State::Unspent));
        assert!(require_unspent(&proofs, std::slice::from_ref(&unspent)).is_ok());
        for state in [State::Spent, State::Pending, State::Reserved] {
            assert!(require_unspent(&proofs, &[ProofState::from((y, state))]).is_err());
        }
        assert!(require_unspent(&proofs, &[]).is_err());
        assert!(require_unspent(&proofs, &[unspent.clone(), unspent]).is_err());
        let other = proof("unexpected", 1000).y().unwrap();
        assert!(require_unspent(&proofs, &[ProofState::from((other, State::Unspent))]).is_err());
    }

    #[test]
    fn bounds_conversion_before_payment_instead_of_redeeming_dust() {
        assert!(validate_conversion_amount(115000, 13).is_err());
        assert!(validate_conversion_amount(115000, 114998).is_ok());
        assert!(validate_conversion_amount(33000, 31999).is_ok());
        assert!(validate_conversion_amount(10000, 9500).is_ok());
        assert!(validate_conversion_amount(10000, 9499).is_err());
        assert!(validate_conversion_amount(0, 0).is_err());
        assert!(validate_conversion_amount(100, 101).is_err());
        assert!(validate_conversion_amount(u64::MAX, u64::MAX).is_ok());
    }
}
