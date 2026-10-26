# Targeted M5 retirement command

`shuestand-m5-recover` is packaged in the backend Docker image. It is deliberately
separate from the generic wallet CLI: no automatic saga recovery, no pooled proof
selection, no withdrawal replay, and no on-chain payout. Destination is fixed to
Shuestand's M7 wallet. M7 pending/sent recipient tokens are not reclaimed.

## What it does

- `plan`: opens M5 SQLite **read-only**, checks all stored Ys with NUT-07, verifies
  old melts are UNPAID and old swaps have zero issued restore signatures. Writes
  a private JSON manifest containing public Ys/state/amount and a store fingerprint.
  It does not read seeds, open a CDK wallet, request invoices or mutate wallet state.
- `execute`: requires the exact freshly revalidated plan, backend-stopped
  acknowledgement, a new recovery directory and an explicit fee ceiling. It makes
  consistent SQLite backups plus seed backups for both wallets before any CDK open.
  It rechecks the plan after backup, opens existing wallets without saga recovery,
  pays only with the enumerated unspent M5 proofs and tracks only returned change
  in subsequent rounds (at most eight). M7 issuance and NUT-07 state are verified.
- Before payment, the command bounds worst-case value loss (including input fees
  and reserve if none is returned). Aggregate loss cannot exceed the configured
  ceiling; that ceiling must itself be no more than 5% of the initial live value.
- Once payments have completed, it rechecks old operation outcomes and current proof
  states, retires confirmed-spent old active proofs to local SPENT, releases any
  still-unspent remainder and removes only the old saga/quote reservation links.
  The complete original records remain in the restricted pre-repair backups.
- Prints M7 credit, exact M5 remainder and value difference. It never deletes live
  dust to fake a zero balance. Historical SPENT records remain as audit history.

Expected from the October 26 audit:115,973 sats live gross (114,980 incident + 993
other value), 26,653 stale active face value. These are not hard-coded expectations:
review the fresh plan; changed totals require investigation. The customer has
already been reimbursed 115,000 sats externally, so never requeue the original withdrawal.

## Deploy the command

On vm-docker, use an updated checkout containing these source changes. Do not run
`git pull` or build an old checkout and assume the new command is present.

```bash
cd /home/marius/shuestand/infra/docker
docker compose build backend
```

Building the image does not run recovery or restart the live backend. Check the
packaged command without launching the backend or connecting a wallet:

```bash
docker compose run --rm --no-deps --entrypoint shuestand-m5-recover backend --help
```

If building outside Docker, use Rust 1.94 and the pinned lockfile:

```bash
cd backend
cargo build --release --locked --bin m5_recover
./target/release/m5_recover --help
```

## Maintenance procedure

Do this in a maintenance window: backend/API will be unavailable. Stop ALL other
wallet CLIs too. `--backend-stopped` is an operator acknowledgement, **not an OS
lock**; this command cannot stop another process from starting the backend.
Do not run multiple copies or allow backend/container auto-recreation during repair.
The helper also exclusively creates `/data/cashu/.m5-recovery.lock` to block another
helper. It removes this guard only after success; failures deliberately leave it in
place. This guard is not honored by the normal backend, hence the stop requirement.

Commands below run from `/home/marius/shuestand/infra/docker` on vm-docker.

### 1. Stop backend and preserve application DB

```bash
docker compose stop backend
RECOVERY_HOST="$PWD/m5-recovery-$(date -u +%Y%m%dT%H%M%SZ)"
mkdir -m 700 "$RECOVERY_HOST"
(umask 077; docker compose exec -T postgres pg_dump -U shuestand -d shuestand -Fc > "$RECOVERY_HOST/application.dump")
```

Confirm `pg_dump` succeeds and the dump is nonempty before continuing. The backend
must stay stopped throughout planning and execution. Retain this directory: it will
contain wallet seeds and spendable secrets in backups; keep it private/offline-backed-up.

The helper runs as the backend image's `shuestand` account, not root. Determine its
UID/GID and grant it access to this new directory (do not chmod the wallet volume):

```bash
wallet_uid="$(docker compose run --rm --no-deps --entrypoint id backend -u)"
wallet_gid="$(docker compose run --rm --no-deps --entrypoint id backend -g)"
sudo chown -R "$wallet_uid:$wallet_gid" "$RECOVERY_HOST"
```

### 2. Create and review a fresh read-only plan

```bash
docker compose run --rm --no-deps \
  -v "$RECOVERY_HOST:/recovery" \
  --entrypoint shuestand-m5-recover backend \
  --wallet-root /data/cashu plan --output /recovery/plan.json
```

Review the printed totals and private manifest. The M7 destination is fixed. Do not
edit the manifest to force execution: source state, fingerprint, amounts and mint
states must match a newly calculated plan exactly. If anything is pending, an old
quote is paid, restore finds signatures, or totals are unexpected, stop and reconcile.

### 3. Execute the reviewed plan

Example ceiling: 5,000 sats. This is a maximum permitted value difference, not an
assertion the operation costs 5,000 sats. The actual fee/change is recorded. For the
115,973-sat inventory, 5,000 is below the enforced 5% upper bound. Choose a lower ceiling
if desired; a ceiling below the required reserve can stop execution before payment.

```bash
docker compose run --rm --no-deps \
  -v "$RECOVERY_HOST:/recovery" \
  --entrypoint shuestand-m5-recover backend \
  --wallet-root /data/cashu execute \
  --plan /recovery/plan.json \
  --recovery-dir /recovery/run \
  --max-fee-sats 5000 \
  --backend-stopped
```

`/recovery/run` MUST NOT already exist. The command creates it privately, with
`m5-wallet.sqlite`, `m7-wallet.sqlite`, seed backups, reviewed plan and append-only
`journal.jsonl`. It never overwrites an old run directory. Keep all of these files.

### 4. Verify, then resume

A successful run ends with `phase: complete` in its journal and prints:
- verified M7 credited sats;
- exact M5 remaining sats (zero only if truly drained);
- aggregate value difference.

Review these figures and the journal before resuming. Nonzero remainder is retained
as UNSPENT value and explicitly reported; it is not a zero-M5 result. Even at zero,
historical SPENT rows remain intentionally archived/auditable. This is a one-time
retirement, not a permanent ban on accepting new M5 customer tokens.

After a successful, reviewed completion:

```bash
docker compose up -d backend
```

Inspect normal backend health and verify operator M7 float/balances against the
journal. Record recovery against the external reimbursement in operator accounting;
this tool does not change withdrawal payout/compensation records.

## If anything fails or times out

**Keep the backend stopped. Retain the complete recovery directory. Do not blindly
rerun, create another run directory to bypass the guard, restore old wallet backups,
or run the generic wallet CLI.** A timeout may occur after payment succeeded.

The journal records quote IDs before melt confirmation, then paid/issuance/credit
phases. Inspect the exact source melt and M7 mint quote, wallet transactions and
stored proofs to determine the next step. A crash after paid melt or M7 issuance is
not automatically resumed by this command. It stops for transaction-specific
reconciliation; old backups are evidence, not automatic rollback for spent money.

New CDK sagas can remain after an interrupted operation. Restarting the backend may
invoke canonical-wallet recovery; do not restart until the interrupted phases have
been reconciled. If planning fails before `/recovery/run` exists, no payment phase has
begun; resolve the plan discrepancy before proceeding. An execute preflight failure
can leave `.m5-recovery.lock` even without a run directory; inspect the error and
prove that execution did not reach any payment before removing that guard.

## Verification performed

- 21 offline Rust tests pass (eight recovery-specific tests, plus 13 existing tests).
- Read-only planner exercised against fresh copied live M5 wallet evidence and live
  state/quote/restore endpoints: 115,973 sats live, 26,653 stale active, no writes.
- Tests cover state ambiguity, exact response coverage, fee/reserve arithmetic,
  fingerprint drift, SQLite read-only enforcement, private complete backups and
  refusal to generate/replace missing/invalid seeds.
- No live melt or cleanup has been run. The payment/issuance path has not been tested
  end-to-end with real staging mints, and the Docker image has not been built here.
  Perform staging exercise/maintenance review before production execute; a successful
  read-only plan is not proof that a paid conversion will succeed.
