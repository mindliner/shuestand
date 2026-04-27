# Shuestand Environment Setup and Tuning Guide

This guide complements `infra/docker/backend.env.example` and follows the same parameter grouping.

Use it as:
1. `cp infra/docker/backend.env.example infra/docker/backend.env`
2. Fill required values first (secrets, wallet, mint, API token)
3. Tune liquidity/limits for your expected traffic
4. Restart backend after env changes (`docker compose restart backend`)

---

## 1) Core service

### `DATABASE_URL`
Postgres DSN for backend state. Keep this stable and backed up.

### `SHUESTAND_BACKEND_PORT`
Backend listen port (default `8080`). Usually unchanged in Docker.

### `RUST_LOG`
Log level, for example `backend=debug` during diagnostics. Use quieter levels in production.

---

## 2) Network + wallet identity

### `BITCOIN_NETWORK`
`bitcoin` for mainnet, `testnet`/`signet` for testing.

### Wallet source (descriptor-only)
- `BITCOIN_DESCRIPTOR`, `BITCOIN_SPEND_DESCRIPTOR`, `BITCOIN_CHANGE_DESCRIPTOR`

To derive descriptors from a seed, use:
`cargo run --bin descriptor_gen -- --network <bitcoin|testnet|signet|regtest> --seed-file /secure/seed.txt --template --output-env /secure/bitcoin-descriptors.env`

Tuning note:
- Prefer descriptors for production lifecycle control and safer key management workflows.

---

## 3) Chain backends (Electrum preferred over Esplora)

### `BITCOIN_ELECTRUM_URL`
Primary chain backend.

### `BITCOIN_ELECTRUM_VALIDATE_DOMAIN`
Keep `true` for TLS hostname validation.

### `BITCOIN_ELECTRUM_SOCKS5`
Optional Tor/proxy route.

### `BITCOIN_ESPLORA_BASE_URL`
Fallback/alternative backend.

Tuning note:
- Keep one primary backend reliable first, then add fallback paths.

---

## 4) Address pool + confirmation policy

### `ADDRESS_POOL_TARGET`
Pre-generated deposit addresses. Increase for higher concurrent kiosk usage.

### `DEPOSIT_TARGET_CONFIRMATIONS`
Required confirmations for Bitcoin→Cashu minting.

### `WITHDRAWAL_TARGET_CONFIRMATIONS`
Confirmations shown/required for withdrawal settlement visibility.

### `CONFIRMATION_POLL_INTERVAL_SECS`
Polling cadence for chain confirmation updates.

Tuning guidance:
- Start balanced: `ADDRESS_POOL_TARGET=20`, `DEPOSIT_TARGET_CONFIRMATIONS=2`, `CONFIRMATION_POLL_INTERVAL_SECS=30`
- Raise confirmations for stronger finality, lower only if UX speed is more important than risk tolerance.

---

## 5) Worker controls

### Withdrawal worker
- `WITHDRAWAL_WORKER_ENABLED`
- `WITHDRAWAL_WORKER_INTERVAL_SECS`
- `WITHDRAWAL_WORKER_MAX_ATTEMPTS`
- `WITHDRAWAL_PAYOUT_FEE_RATE_VB` (manual override)

### Deposit worker
- `DEPOSIT_WORKER_ENABLED`
- `DEPOSIT_WORKER_INTERVAL_SECS`
- `DEPOSIT_WORKER_MAX_ATTEMPTS`

Tuning guidance:
- Faster loops (`10-15s`) improve responsiveness but increase backend and chain-backend load.
- Keep retry counts moderate, investigate persistent failures instead of masking them with high attempts.

---

## 6) Cashu mint + local wallet

### `CASHU_MINT_URL`
Canonical mint URL for issued/received ecash.

### `CASHU_WALLET_DIR`
Persistent wallet storage path inside container/host.

### `PUBLIC_BASE_URL`
Public HTTPS base URL for callback targets (NUT-18 transport).

Critical note:
- `PUBLIC_BASE_URL` must be externally reachable over HTTPS for mobile/off-LAN wallets.

---

## 7) Operator/API security

### `WALLET_API_TOKEN`
Required operator/API auth secret. Use a long random value.

### `TRUST_PROXY_HEADERS`
Enable only behind a trusted reverse proxy that sanitizes forwarding headers.

### `CORS_ALLOWED_ORIGINS`
Explicit frontend origin allowlist.

Tuning guidance:
- Default-deny posture: only trusted origins and trusted proxy paths.

---

## 8) Float management

### `FLOAT_TARGET_SATS`
Target total float planning anchor.

### `FLOAT_MIN_RATIO` / `FLOAT_MAX_RATIO`
Acceptable balance band vs target.

### `FLOAT_GUARD_INTERVAL_SECS`
Float monitoring cadence.

### `FLOAT_DRIFT_ALERT_RATIO`
Alert threshold for sudden float drift.

Tuning guidance:
- Smaller bands = earlier alerts, more noise.
- Larger bands = less noise, slower operator reaction.

---

## 9) Transaction sizing + anti-abuse controls

### `SINGLE_REQUEST_CAP_RATIO`
Max per-request share of available float.

### `PENDING_DEPOSIT_TTL_SECS`
How long pending/partial deposits reserve capacity.

### `MAX_PENDING_DEPOSITS_PER_SESSION`
Caps outstanding pending pressure per session.

### Minimums and buffers
- `DEPOSIT_MIN_SATS`
- `WITHDRAWAL_MIN_SATS`
- `WITHDRAWAL_FEE_BUFFER_SATS`

### Practical starter profile (small launch)
For a conservative first production rollout:
- `FLOAT_TARGET_SATS=500000`
- `SINGLE_REQUEST_CAP_RATIO=0.15`
- `PENDING_DEPOSIT_TTL_SECS=600`
- `MAX_PENDING_DEPOSITS_PER_SESSION=2`
- `DEPOSIT_MIN_SATS=25000`
- `WITHDRAWAL_MIN_SATS=25000`

Interpretation:
- At 500k target and 15% cap, a single request is limited to ~75k sats.
- TTL controls how long abuse/abandonment can tie up float.

---

## 10) Fee estimator (optional bitcoind RPC)

### RPC connection
- `BITCOIND_RPC_URL`
- `BITCOIND_RPC_USER`
- `BITCOIND_RPC_PASSWORD`

### Estimator behavior
- `FEE_ESTIMATOR_REFRESH_SECS`
- `FEE_ESTIMATOR_FAST_BLOCKS`
- `FEE_ESTIMATOR_ECONOMY_BLOCKS`
- `FEE_ESTIMATOR_MIN_SAT_PER_VB`
- `FEE_ESTIMATOR_MAX_SAT_PER_VB`

Tuning guidance:
- Use sane min/max clamps to avoid pathological fee spikes or stale low estimates.

---

## 11) Optional webhooks

### `FLOAT_ALERT_WEBHOOK_URL`
Receives float band and drift events.

### `TRANSACTION_WEBHOOK_URL`
Receives monotonic completed-transaction counter updates.

### `SECURITY_ALERT_WEBHOOK_URL`
Receives security-related event alerts.

Tuning guidance:
- Point webhooks to durable automation endpoints (n8n, Slack bot, PagerDuty bridge).
- Keep handlers idempotent.

---

## Preset profiles (small / medium / high throughput)

Use these as starting points, then adjust from live data.

### Small (single kiosk, low concurrency)
- `FLOAT_TARGET_SATS=500000`
- `SINGLE_REQUEST_CAP_RATIO=0.15`
- `PENDING_DEPOSIT_TTL_SECS=600`
- `MAX_PENDING_DEPOSITS_PER_SESSION=2`
- `ADDRESS_POOL_TARGET=20`
- `DEPOSIT_TARGET_CONFIRMATIONS=2`
- `WITHDRAWAL_WORKER_INTERVAL_SECS=15`
- `DEPOSIT_WORKER_INTERVAL_SECS=10`

Capacity intuition: about 75k sats max per request.

### Medium (multiple kiosks, moderate concurrency)
- `FLOAT_TARGET_SATS=2000000`
- `SINGLE_REQUEST_CAP_RATIO=0.12`
- `PENDING_DEPOSIT_TTL_SECS=480`
- `MAX_PENDING_DEPOSITS_PER_SESSION=2`
- `ADDRESS_POOL_TARGET=50`
- `DEPOSIT_TARGET_CONFIRMATIONS=2`
- `WITHDRAWAL_WORKER_INTERVAL_SECS=12`
- `DEPOSIT_WORKER_INTERVAL_SECS=8`

Capacity intuition: about 240k sats max per request.

### High (event/peak traffic, strong operator monitoring)
- `FLOAT_TARGET_SATS=5000000`
- `SINGLE_REQUEST_CAP_RATIO=0.10`
- `PENDING_DEPOSIT_TTL_SECS=360`
- `MAX_PENDING_DEPOSITS_PER_SESSION=1`
- `ADDRESS_POOL_TARGET=120`
- `DEPOSIT_TARGET_CONFIRMATIONS=2` (or `3` for stricter finality)
- `WITHDRAWAL_WORKER_INTERVAL_SECS=10`
- `DEPOSIT_WORKER_INTERVAL_SECS=6`

Capacity intuition: about 500k sats max per request.

### How to choose quickly
- If you value abuse resistance over convenience, lower `SINGLE_REQUEST_CAP_RATIO` and shorter `PENDING_DEPOSIT_TTL_SECS`.
- If users frequently queue behind address generation, raise `ADDRESS_POOL_TARGET`.
- If chain fees/latency are unstable, keep worker intervals conservative and avoid over-tight polling.

---

## Change management checklist

Before changing env in production:
1. Save current `backend.env`
2. Edit only required keys
3. Restart backend service
4. Verify `/api/v1/config`, operator panel, and one end-to-end deposit+withdrawal smoke test
5. Watch logs/alerts for at least one traffic cycle
