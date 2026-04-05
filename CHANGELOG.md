# Changelog

## 2026-04-05
- Added transaction webhook (`TRANSACTION_WEBHOOK_URL`)
- Added information about the canonical mint to the kios so users understand where the cashu token come from
## 2026-04-02
- Added optional webhook alerts (`FLOAT_ALERT_WEBHOOK_URL`) so float band/drift transitions emit JSON events (easy to feed into n8n or any monitoring pipeline).
- Made the Bitcoin→Cashu minimum configurable via `DEPOSIT_MIN_SATS`, exposed it through `/api/v1/config`, and taught the kiosk to hydrate the runtime deposit + withdrawal floors (with better deposit-flow disable messaging).
- Reworked the kiosk chrome: the active-session frame now spans the full width with the flow toggle + backend endpoint tucked inside, the landing copy reflects the “Shuestand: Privacy Focused B<->C” branding, and the README install instructions match the current Docker port mapping.

## 2026-03-29
- Added detailed explanations to environment parameters (backend.env)
- Fixed a bug with processing payment requests (now testing positively with Minibits and Macadamia).
- Implemented automatic withdrawal progression to state "settled" after confs have been reached.
- Kiosk polish: renamed "Forget this withdrawal" into "Archive this withdrawal", brought some buttons into conforming with standard design.
## 2026-03-27
- Added anonymous work-session support: `POST /api/v1/sessions` mints session IDs + hashed tokens, `/sessions/resume` reissues tokens from the four-block claim code, and every kiosk deposit/withdraw now tags its `session_id`.
- Kiosk UI shows a session panel (start/resume/claim-code copy), stores tokens per-session, and automatically attaches `X-Shuestand-Session` on all kiosk API calls; local tracking lists are now namespaced per session so browsers can resume safely.
- Same-browser resumes now rehydrate tracked deposits/withdrawals reliably and the operator panel calls those queues “Ongoing” instead of “Stuck.”

## 2026-03-24
- Added per-deposit pickup tokens + resume codes so the kiosk halts before exposing a token and lets users/operators resume delivery later (with clipboard helpers baked into the reveal button).
- Implemented webhook delivery for any `http(s)` delivery hint; successful callbacks auto-fulfill the deposit, failures are recorded so the flow falls back to manual pickup.
- Extended the operator console with deposit/withdrawal cleanup actions (settle/fail/archive/requeue) and surfaced delivery errors directly in the kiosk status card.

## 2026-03-20
- Added the shared wallet module + operator CLI (`invoice`, `send`, `receive`, `pay`) so float management no longer depends on the Nutshell/Python tooling.
- Split the backend into dedicated modules (`config`, `db`, `http`, `workers`) and rewired `main.rs` to just orchestrate config, workers, and server bootstrap.
- Introduced `src/bin/wallet.rs` and documented the new commands in the README.
