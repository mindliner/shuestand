# shuestand API draft (v0)

Base URL (dev): `http://localhost:8080`

## Common conventions
- Content-Type: `application/json`.
- All responses wrap payloads in `{ "data": ... }` and errors in `{ "error": { "code": string, "message": string } }`.
- Amounts are integers denominated in **satoshis**.
- `delivery_hint` is an optional string that can hold either a Cashu wallet URL (`cashu://`, `nut://`) or an opaque label understood by upstream systems (e.g., upstream order or session IDs).

### Work sessions
- Kiosk clients SHOULD start or resume an anonymous work session before creating deposits/withdrawals.
- `POST /api/v1/sessions` returns `{ session_id, token, claim_code, expires_at }`. The `token` must be echoed on every kiosk request via the `X-Shuestand-Session` header; the claim code is a four-block string users can jot down to resume.
- `POST /api/v1/sessions/resume` accepts `{ "claim_code": "ABCD-EFGH-..." }` and returns the same payload as `start`, re-issuing the header token if the session is still active.
- Deposits/withdrawals created with a session are only visible to callers presenting the matching `X-Shuestand-Session` header. Operator endpoints (guarded by `WALLET_API_TOKEN`) remain global.

## Entities

### Deposit
Represents an on-chain Bitcoin funding flow that mints Cashu tokens once the transaction reaches the configured confirmation target.

```json
{
  "id": "dep_01hxfdsy2vk65y1etv7c0h1p50",
  "amount_sats": 50000,
  "state": "confirming",
  "delivery_hint": "cashu://wallet/minibits",
  "address": "bc1q...",
  "txid": "c06a…",
  "confirmations": 2,
  "target_confirmations": 3,
  "last_checked_at": "2026-03-18T19:23:41Z",
  "last_event": "2026-03-18T19:22:31Z"
}
```

Additional fields:
- `txid` – first transaction detected paying the derived address (optional until first seen).
- `confirmations` – watcher-maintained depth from the current chain tip.
- `last_checked_at` – ISO timestamp of the most recent watcher poll.

`state` enum:
- `pending` – address allocated, awaiting first seen.
- `confirming` – first seen; waiting for `target_confirmations`.
- `minting` – confirmations met; mint call in progress.
- `delivering` – token being pushed to wallet/webhook.
- `ready` – token available for pickup (includes the token blob / QR metadata).
- `fulfilled` – delivery succeeded (wallet push, webhook ack, or manual pickup recorded).
- `failed` – unrecoverable error (details appended to `notes`).

### Withdrawal
Represents Cashu → Bitcoin redemption via on-chain payout.

```json
{
  "id": "wd_01hxferwct2qad9cgdjsv1q7wx",
  "state": "queued",
  "token_value_sats": 3000,
  "delivery_address": "bc1q...",
  "fee_quote_sats": 142,
  "txid": null,
  "last_event": "2026-03-18T19:25:01Z"
}
```

`state` enum:
- `queued` – token validated, waiting for operator policy checks.
- `broadcasting` – transaction building/broadcasting.
- `confirming` – tx sent, waiting for 1+ confirmation.
- `settled` – payout final.
- `failed` – error occurred (insufficient funds, invalid token, etc.).

## Endpoints

### POST `/api/v1/sessions`
Request body: _empty_.

Response `200 OK`:
```json
{
  "data": {
    "session_id": "sess_01hy...",
    "token": "st_d4f2...",
    "claim_code": "ABCD-EFGH-IJKL-MNOP",
    "expires_at": "2026-03-27T16:30:00Z"
  }
}
```

Clients must stash the `token` and send it as `X-Shuestand-Session` on every kiosk request; the claim code is a user-friendly representation for jotting down/resuming.

### POST `/api/v1/sessions/resume`
Request body:
```json
{ "claim_code": "ABCD-EFGH-IJKL-MNOP" }
```

Response mirrors the start endpoint, re-issuing the current token/expiry when the session is still valid.

### POST `/api/v1/deposits`
Request body:
```json
{
  "amount_sats": 75000,
  "delivery_hint": "nut://app.minibits.cash/lnurldevice",
  "metadata": {
    "order_id": "np_12345",
    "note": "booth-7"
  }
}
```

Response `201 Created`:
```json
{
  "data": {
    "id": "dep_01hxf...",
    "address": "bc1q...",
    "state": "pending",
    "target_confirmations": 3
  }
}
```

### GET `/api/v1/deposits/{id}`
Returns the full deposit object plus (when `state === "ready"`) a `token` field containing the Cashu token string and a `token_qr` payload for display. When `delivery_hint` refers to a wallet URL, the backend will attempt a push first and still return the token for fallback.

### POST `/api/v1/withdrawals`
Request body:
```json
{
  "token": "cashuA1...",
  "delivery_address": "bc1q...",
  "max_fee_sats": 500
}
```

Response `202 Accepted`:
```json
{
  "data": {
    "id": "wd_01hx...",
    "state": "queued"
  }
}
```

### GET `/api/v1/withdrawals/{id}`
Returns the withdrawal object including `txid`, `fee_paid_sats`, and `state` progression.

## Webhooks / Integrations
- **Webhook delivery**: when `delivery_hint` is an `http(s)` URL, the backend POSTs `{ deposit_id, amount_sats, token, txid?, hint }` as soon as the token is minted. Any non-2xx response leaves the deposit in `ready` with the failure recorded so an operator/guest can fall back to manual pickup.
- **Wallet push**: when `delivery_hint` is a wallet deeplink (`cashu://`, `nut://`), we surface it in the kiosk so the user can hand off to the native app; automatic pushes for those targets are still TBD per wallet spec.

## Environment knobs
|              Variable             |                                      Purpose                                       |                Default                 |
|-----------------------------------|------------------------------------------------------------------------------------|----------------------------------------|
| `BITCOIN_DESCRIPTOR`              | Descriptor used to derive kiosk deposit addresses. Mandatory for real deployments. | _None (falls back to mock addresses)_  |
| `BITCOIN_NETWORK`                 | `bitcoin`, `testnet`, `signet`, or `regtest`.                                      | `regtest`                              |
| `BITCOIN_ESPLORA_BASE_URL`        | Base URL for an Esplora-compatible API the watcher can poll.                       | _None (disables confirmation tracker)_ |
| `ADDRESS_POOL_TARGET`             | Minimum count of pre-derived addresses kept in the ready pool.                     | `20`                                   |
| `DEPOSIT_TARGET_CONFIRMATIONS`    | How many confirmations a deposit must reach before minting.                        | `3`                                    |
| `CONFIRMATION_POLL_INTERVAL_SECS` | How often the watcher polls the chain for updates.                                 | `30`                                   |

## Error envelope
```json
{
  "error": {
    "code": "validation_error",
    "message": "amount_sats must be between 50,000 and 2,000,000"
  }
}
```

---
This document will evolve as we implement storage, auth, and policy controls, but it gives the frontend + integrations team a stable contract to begin with.
