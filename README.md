# shuestand

> Bidirectional Bitcoin ⇄ Cashu bridge desk for events, kiosks, and pop-up sats stands.

## TL;DR
Shuestand lets users fund a Cashu wallet from on-chain Bitcoin or withdraw sats back out, all through a single, kiosk-friendly interface. The frontend borrows the polished flow we developed for Catofa, while the backend reuses Lakeside's Cashu plumbing, adds on-chain watchers, and sits on top of our trusted infrastructure stack:

|      Layer      |                        Default choice                       |
|-----------------|-------------------------------------------------------------|
| Lightning node  | `Open_Hand`                                                 |
| Cashu mint      | `https://m7.mountainlake.io`                                |
| On-chain wallet | New descriptor-based hot wallet (sweepable to cold storage) |
| Frontend        | React/TypeScript (Catofa DNA)                               |
| Backend         | Rust service orchestrating Bitcoin, Cashu, and Lightning    |

## User Flows
1. **Bitcoin → Cashu**
   - User requests a deposit, gets a unique on-chain address (BIP21 URI + QR).
   - Chain watcher tracks mempool + confirmation depth; status is streamed to the UI with confirmation countdowns.
   - Once the target confirmations clear, the backend mints tokens via the default mint. If the `delivery_hint` is an `http(s)` webhook we auto-post the token there; otherwise the kiosk shows a pickup screen (with resume code + clipboard helper) so the guest/operator can claim it later.

2. **Cashu → Bitcoin**
   - User pastes/imports a Cashu token, sees a quote (amount, projected miner fee, eta).
   - Funds are redeemed at the mint, credited to our float, and the backend crafts/broadcasts an on-chain payout from the hot wallet (no Lightning dependency).
   - UI shows progress, payout txid + confirmations, and an audit trail for operators.

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
- **Lightning adaptor**: LND/CLN wrapper for invoice creation, payment, keysend; auto-balancing policies for `Open_Hand` channels.
- **Cashu mint client**: Nutshell-compatible RPC with token proof validation, minted-token storage, and redemption queue.
- **Delivery automation**: per-deposit resume/pickup tokens plus webhook delivery for any `http(s)` hints so kiosks can push tokens directly into upstream systems when configured.
- **Risk + compliance guardrails**: rate limits, per-user caps, configurable fees/spreads, and operator alerts when reserve ratios drift.

### Address pool & chain watcher
The backend now pre-derives kiosk deposit addresses from a descriptor, keeps a hot pool ready for allocation, and watches the chain for first-seen + confirmation depth:

| Component | Details |
|-----------|---------|
| Address source | `BITCOIN_DESCRIPTOR` (any valid descriptor understood by BDK). Falls back to deterministic mock addresses if unset. |
| Network | `BITCOIN_NETWORK` (`regtest` default). |
| Chain watcher | Esplora-compatible REST endpoint (`BITCOIN_ESPLORA_BASE_URL`). Tracks tip height + per-address transactions to update deposit states and confirmation counts. |
| Pool sizing | `ADDRESS_POOL_TARGET` (default 20) and a background refill loop every 60s keep the pool topped up. |
| Poll cadence | `CONFIRMATION_POLL_INTERVAL_SECS` (default 30s). |

When the watcher sees a transaction hit one of our addresses it stamps the deposit with the txid, increments confirmations as new blocks arrive, and automatically transitions from `pending → confirming → minting` once the configured target depth is reached. Addresses keep the txid + confirmation metadata for operator audit trails.

## Infrastructure Defaults
1. **`Open_Hand` Lightning node**
   - Primary for payouts (Cashu → BTC) and optional fast deposits.
   - Needs channel liquidity monitoring + autopilot or manual policies.
2. **`m7.mountainlake.io` mint**
   - Initial Cashu mint for issuance/redemption.
   - Keep the option open to spin up our own mint later for full reserve control.
3. **Hot wallet**
   - Descriptor-based (BIP84) wallet dedicated to shuestand flows.
   - Enforce max float; automatic sweeps to multi-sig cold storage.
4. **Observability**
   - Prometheus metrics (pending tx count, float ratios, LN liquidity, mint latency).
   - Structured logs shipped to Loki/Elastic.
   - Alerting hooks (PagerDuty/ntfy) for stuck transactions or reserve breaches.

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

### Cross-mint swaps
- Foreign Cashu tokens are now imported into per-mint CDK wallets, melted via Lightning to the kiosk mint, and the resulting proofs are minted before we touch the on-chain payout.
- The swapper fetches the melt quote upfront, checks `invoice + fee_reserve + input_fee` against the wallet’s spendable proofs, and shrinks the invoice before any proofs are reserved so we don't burn float chasing an impossible payment.
- `MintQuoteState::Paid` is treated as success, so as soon as CLN marks the kiosk invoice paid we pull the canonical proofs and move the withdrawal to the on-chain stage.

## Roadmap (Draft)
1. **Spec & scaffolding** (Week 1)
   - Finalize UX mockups, data contracts, and state diagrams.
   - Set up repo, CI, formatting, and type-check hooks.
2. **Backend foundations** (Weeks 2-3)
   - Chain watcher + address pool, Lightning adaptor, Cashu client.
   - Persistence layer and integration tests for both flows.
3. **Frontend build-out** (Week 4)
   - React screens, session state, QR/token helpers, real-time updates.
4. **Settlement safety & ops** (Week 5)
   - Float accounting, sweeps, monitoring, operator dashboard.
5. **Pilot & polish** (Week 6)
   - Dogfood with our own wallets, tighten copy, add branding knobs, prep deployment guides (Docker/systemd/Ansible).

## Planned Repo Layout
```
shuestand/
├── README.md
├── backend/
│   ├── Cargo.toml
│   └── src/
├── frontend/
│   ├── package.json
│   └── src/
├── infra/
│   ├── docker/
│   └── ansible/
└── docs/
    └── specs/
```

## Docker deployment (preview)
1. `cp infra/docker/backend.env.example infra/docker/backend.env` and fill in mainnet secrets (descriptors, mint URL, Esplora endpoint, wallet token, etc.).
2. `docker compose -f infra/docker/docker-compose.yml up -d --build` to launch Postgres + backend + nginx-served frontend.
3. Apply the database schema inside the backend container once: `docker compose run --rm backend sqlx migrate run`.
4. Visit `http://localhost:8080` for the kiosk/operator UI; `/api` requests are reverse-proxied to the backend container.

Once running, update `infra/docker/backend.env` whenever you rotate keys/policies and restart the backend service (`docker compose restart backend`).

## Publishing behind a reverse proxy
When the Docker stack lives on an internal host (e.g., `vm-docker:8872`), expose it through a public reverse proxy so visitors reach it via HTTPS without touching the internal network.

1. **Pick the public domain + certificate.** Request a Let’s Encrypt cert (webroot or DNS) for the hostname you plan to expose (e.g., `shuestand.mountainlake.io`).
2. **Create an upstream + redirect block.** On the proxy host (nginx example), add a server that forwards both the kiosk assets and `/api` calls to the compose frontend listener:

   ```nginx
   upstream shuestand_frontend {
       server vm-docker:8872;
   }

   server {
       listen 80;
       listen [::]:80;
       server_name shuestand.mountainlake.io;

       location /.well-known/acme-challenge/ {
           root /var/www/certbot;
       }

       return 301 https://$host$request_uri;
   }

   server {
       listen 443 ssl;
       listen [::]:443 ssl;
       server_name shuestand.mountainlake.io;

       ssl_certificate     /etc/letsencrypt/live/shuestand.mountainlake.io/fullchain.pem;
       ssl_certificate_key /etc/letsencrypt/live/shuestand.mountainlake.io/privkey.pem;
       include /etc/letsencrypt/options-ssl-nginx.conf;
       ssl_dhparam /etc/letsencrypt/ssl-dhparams.pem;

       location / {
           proxy_pass http://shuestand_frontend;
           include /etc/nginx/snippets/ssl-proxy-params.conf; # sets Host/X-Forwarded-* headers
       }
   }
   ```

3. **Reload nginx and verify HTTPS.** Visit `https://<domain>` and confirm that the kiosk loads and that the Nut18 QR transports point to the HTTPS host (the backend derives the callback URL from `Host` + `X-Forwarded-Proto`). Avoid double-proxy stacks that overwrite `X-Forwarded-Proto` with `http`, otherwise Cashu wallets will refuse to POST back to the funding endpoint.

## Development Notes
- Rust nightly not required; stick to stable + Clippy + fmt in CI.
- Frontend uses pnpm + TypeScript strict mode.
- Secrets (mint keys, node macaroon, wallet descriptors) stay outside the repo—use `.envrc` + `age`/`sops` for local dev.
- Testing strategy: unit tests for proof/tx logic, integration tests against regtest bitcoind + Cashu mint docker compose, and Cypress for end-to-end kiosk flows.

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

Use the Rust CLI to manage float without touching the Python/Nutshell tools:

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
- Provide a Postgres DSN via `DATABASE_URL` (e.g., `postgres://user:pass@vm-openhand:5432/shuestand`).
- Run `cargo check` or start the backend to apply the `sqlx` migrations from `backend/migrations`.
- No local SQLite file is needed anymore; remove any leftover `shuestand.db` artifacts.

## Documentation
- [`docs/api.md`](docs/api.md): REST contract for deposits, withdrawals, delivery hints, and webhook callbacks.

Let's iterate on this README as we lock in UX comps and deployment constraints.
