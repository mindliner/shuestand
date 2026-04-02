## Shuestand Development Notes

### Backend code layout
- `backend/src/config.rs` – environment parsing and runtime knobs.
- `backend/src/db.rs` – SQLx database access layer + deposit/withdraw/address models.
- `backend/src/http.rs` – Axum router + REST handlers/DTOs (`http::router(state)`).
- `backend/src/workers/{deposit,withdrawal}.rs` – background loops for minting tokens and redeeming withdrawals.
- `backend/src/wallet/mod.rs` + `src/bin/wallet.rs` – shared Cashu wallet plumbing and the operator CLI that manages float.

### Wallet operations CLI
Environment variables for the on-chain wallet:

- `BITCOIN_SPEND_DESCRIPTOR` – descriptor with signing keys (private) for external addresses.
- `BITCOIN_CHANGE_DESCRIPTOR` – optional change descriptor (defaults to the spend descriptor when omitted).
- `BITCOIN_ESPLORA_BASE_URL` – your Esplora endpoint (e.g., https://electrs.mountainlake.io).

Use the operator mode of the front-end or the Rust CLI to manage float:

```bash
# Inspect balances
cargo run --bin wallet -- balance

# Request an invoice (Bolt11 by default, add --bolt12 for offers)
cargo run --bin wallet -- invoice --amount 75000

# Export proofs
cargo run --bin wallet -- send 25000 --output /tmp/token.txt

# Receive proofs from a file or inline token
cargo run --bin wallet -- receive --file /tmp/token.txt

# Pay/melt a Lightning invoice
cargo run --bin wallet -- pay lnbc1...
```

### Database
- Provide a Postgres DSN via `DATABASE_URL` (e.g., `postgres://user:pass@hostname:5432/shuestand`).
- Run `cargo check` or start the backend to apply the `sqlx` migrations from `backend/migrations`.
- No local SQLite file is needed anymore; remove any leftover `shuestand.db` artifacts.

## Documentation
- [`docs/api.md`](docs/api.md): REST contract for deposits, withdrawals, delivery hints, and webhook callbacks.

### Work sessions & claim codes
- Every kiosk browser runs inside an anonymous work session. Click “Start session” to mint a session token + four-block claim code; the token is kept in `sessionStorage` and attached to all `/api/v1/deposits|withdrawals` calls as `X-Shuestand-Session`.
- The claim code is the human-friendly representation of that token. Jot it down or export the QR so you can resume later (same browser = instant restore, different browser = re-enter any dep/wd IDs you care about until we ship the session inbox).
- Ending a session immediately pauses polling but does **not** cancel deposits/withdrawals. Resuming with the claim code rehydrates the tracked IDs on that browser profile and picks up status polling right where you left off.
- Operator view continues to see all deposits/withdrawals regardless of session and can help a guest recover an ID if needed.

## System Architecture
- **Frontend** (React + Vite): dual-pane interface, live status toasts, QR helpers, token import/export modal, activity log.
- **Gateway API** (Rust / Axum):
  - REST + WebSocket endpoints for session orchestration.
  - Stateless auth tokens per kiosk/session plus optional operator login.
  - Structured logging + OpenTelemetry spans.
  - Binds to `0.0.0.0:8080` by default; set `SHUESTAND_BACKEND_PORT` to override.
- **Ledger / DB** (SQLite for dev, Postgres in prod): tracks deposits, redemptions, float exposure (on-chain, LN, Cashu liabilities), and audit signatures.
- **Bitcoin subsystem**: bitcoind/electrs client, address manager, confirmation oracle, rebroadcaster, fee estimator.
- **Cashu mint client**: Nutshell-compatible RPC with token proof validation, minted-token storage, and redemption queue.
- **Delivery automation**: per-deposit resume/pickup tokens plus webhook delivery for any `http(s)` hints so kiosks can push tokens directly into upstream systems when configured.
- **Risk + compliance guardrails**: rate limits, per-user caps, configurable fees/spreads, and operator alerts when reserve ratios drift.

### Address pool & chain watcher
The backend now pre-derives kiosk deposit addresses from a descriptor, keeps a hot pool ready for allocation, and watches the chain for first-seen + confirmation depth:

|   Component    |                                                                            Details                                                                            |
|----------------|------------------------------|
| Address source | `BITCOIN_DESCRIPTOR` (any valid descriptor understood by BDK). Falls back to deterministic mock addresses if unset.                                           |
| Network        | `BITCOIN_NETWORK` (`regtest` default).                                                                                                                        |
| Chain watcher  | Esplora-compatible REST endpoint (`BITCOIN_ESPLORA_BASE_URL`). Tracks tip height + per-address transactions to update deposit states and confirmation counts. |
| Pool sizing    | `ADDRESS_POOL_TARGET` (default 20) and a background refill loop every 60s keep the pool topped up.                                                            |
| Poll cadence   | `CONFIRMATION_POLL_INTERVAL_SECS` (default 30s).                                                                                                              |

When the watcher sees a transaction hit one of our addresses it stamps the deposit with the txid, increments confirmations as new blocks arrive, and automatically transitions from `pending → confirming → minting` once the configured target depth is reached. Addresses keep the txid + confirmation metadata for operator audit trails.

### Cashu redeemer worker
The backend now embeds the CDK wallet to redeem incoming Cashu tokens inside the withdrawal worker. Configure it through environment variables:

| Variable | Purpose |
|----------|---------|
| `WITHDRAWAL_WORKER_ENABLED` | Turn the worker on/off without redeploying. Default: `true`. |
| `WITHDRAWAL_WORKER_INTERVAL_SECS` | Poll cadence for queued withdrawals (default `15`). |
| `WITHDRAWAL_WORKER_MAX_ATTEMPTS` | Retry budget before a withdrawal is marked `failed` (default `5`). |
| `CASHU_MINT_URL` | Nutshell-compatible mint URL (e.g., `https://m7.mountainlake.io`). Required for real redemptions. |
| `CASHU_WALLET_DIR` | Optional override for the CDK wallet/seed location. Defaults to `~/.shuestand/cashu`. |

On startup the service (1) derives/loads a 64-byte seed under `CASHU_WALLET_DIR`, (2) opens `wallet.sqlite` via `cdk-sqlite`, (3) replays incomplete redemption sagas, and (4) hands the wallet to the withdrawal worker. Each redemption logs the amount, updates `withdrawals.token_value_sats`, and either advances to `broadcasting` or retries until the attempt budget is exhausted. See `backend/.env.mainnet.example` for a production-ready env stencil you can copy and fill with real descriptors/credentials.
