# Changelog

## 2026-03-29
- implemented automatic withdrawal progression to state "settled" after confs have been reached
- Kiosk polish: renamed "Forget this withdrawal" into "Archive this withdrawal", brought some buttons into conforming with standard design
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
