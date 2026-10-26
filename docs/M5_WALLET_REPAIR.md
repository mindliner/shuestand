# M5 wallet safety repair — 2026-10-26

## Local code changes

- A spent-token receive error is an error, not evidence of a previous import.
  Retries never substitute pooled wallet funds for this customer's token.
- Receive snapshots and new-output capture share one wallet mutex. Only newly
  issued proofs whose total equals CDK's received amount fund the conversion.
- NUT-07 responses must exactly cover those proofs, without duplicate or extra
  Ys, and all must be UNSPENT. Missing/ambiguous/pending/spent states stop payment.
- Use CDK `prepare_melt_proofs`, not pooled `prepare_melt`. Old wallet proofs
  cannot be selected for a new token's conversion. A melt must return PAID and
  canonical proofs must match the invoice before the conversion succeeds.
- No balance-based shrink/retry after attempting melt preparation. Only quote
  sizing before payment is allowed, using the same transaction-specific proofs.
- Refuse conversion loss greater than 5% of original token face value before
  attempting payment. This is a conservative safety ceiling, not a fee quote;
  it includes receive input fees and the invoice reduction for the melt reserve.
  Returned foreign change is not independently accounted as a charged fee here.
- Payout executors reject zero/dust-sized or unaccounted shortfalls both on first
  redemption and on `token_consumed` retries. Bounded shortfalls must match the
  stored swap fee. Canonical receives also report their input-fee difference.
- Foreign wallet opening no longer invokes bulk CDK saga recovery. Canonical
  wallet recovery remains enabled and now surfaces errors rather than ignoring
  them.

## Failure/recovery behavior

This is fail-closed, not a new automatic recovery engine. If a process stops
between receive, melt, canonical issuance and recording consumption, a retry
may stop with reconciliation required. CDK persists its wallet/saga evidence,
while a spent-token error alone is deliberately insufficient to resume payment.
Durable end-to-end resumable conversion receipts remain a follow-up improvement.
Do not describe this patch as guaranteeing automatic recovery after crashes or
as solving the separate on-chain broadcast/DB-commit idempotency window.

NUT-07 uses CDK's `check_proofs_spent`: the SDK removes confirmed-spent submitted
proofs from its local wallet. The new live path submits only this conversion's
proofs; it does not sweep or release existing reserved wallet proofs. No live
wallet was opened or modified during development/testing of this patch.

## Verification

- `cargo test --locked --all-targets`: 13 passed, including 7 new regression tests.
- Regressions cover old/unrelated proof exclusion, missing receive outputs,
  spent/pending/malformed state replies, bounded fees and the exact 115000 -> 13
  incident, including an executor test with a closed DB pool proving validation
  occurs before a consumption write and on a persisted-consumed retry.
- `cargo fmt --check` and `git diff --check`: pass.
- Tests are offline synthetic proofs/mocked redemption; no live mint, staging
  swap or production payout has been exercised. Existing SQLx future-compatibility
  warning remains outside this patch.

## Rollout and existing wallet reconciliation

1. Review backend-only changes; do not inadvertently include the separate local
   frontend/system-theme work. Do not combine with a CDK/SQLx schema migration.
2. On staging, test real canonical and foreign conversions with ordinary fees,
   a preloaded stale/unrelated wallet balance, spent-token replay, and interrupted
   receive/melt/canonical issuance. Verify no foreign balance fallback and no
   smaller payout on failure; keep production/customer tokens out of staging.
3. Before production wallet work, pause new conversions and take consistent,
   access-restricted backups of the app database and complete CDK wallet stores
   (including SQLite WAL/SHM as applicable). Preserve the incident evidence.
4. Re-query current mint states before any repair; October 26's reconciliation
   is not authority to assume states are unchanged. Inventory by mint and saga.
   Keep pending and saga-owned reserved proofs quarantined. Do not run bulk
   restore, reset quotes or indiscriminately unreserve proofs.
5. Reconcile confirmed-spent old proofs using their mint states and operation
   evidence; retain an audit manifest. Prepare exact transaction-specific
   changes for still-unspent replacement proofs. No production repair commands
   or automatic deletion/unreserve script are included in this patch.
6. The 115000-sat customer has already been reimbursed out of band. Do not
   requeue wd_297f3c24-0cc3-4f6e-974f-1b4915896714 for customer payment. The last
   verified 114980 sats remain reserved/unspent; recovery must return value to
   operator-controlled accounting without a duplicate customer payout. Trace
   the separate 18-sat spent subset/change independently.

This repair has not been deployed. Existing production reconciliation and
targeted recovery have not been executed.
