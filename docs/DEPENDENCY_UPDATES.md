# Dependency refresh — 2026-10-25

## Scope

Local source update only. No production deployment, database migrations, wallet
restore, proof-state changes, or live swap tests were performed.

### Backend

- Refresh Cargo.lock within existing compatible version ranges using Rust 1.94.
- Upgrade Axum 0.7.9 → 0.8.9, tower-http 0.5.2 → 0.7.1,
  BDK 0.29.0 → 0.30.2, and direct thiserror 1 → 2.0.21.
- Convert Axum route captures from `:id` to `{id}` without changing public URLs.
- Add a test constructing the actual API route table: legacy route patterns
  panic at registration time under Axum 0.8, so compilation alone is insufficient.
- Declare Rust 1.94 as the minimum, matching the existing Docker builder,
  and require `cargo build --release --locked` in that builder.

### Frontend

- Update Cashu SDK 4.0.0 → 4.12.0, React/React DOM 19.2.4 → 19.3.1,
  React Router 7.13.2 → 7.18.4, Vite 8.0.8 → 8.3.5, and compatible
  query, hashing, encoding, type-definition, and plugin dependencies.
- Upgrade TypeScript 5.9.3 → 6.0.3 and ESLint 9.39.4 → 10.13.0.
- Use Node 24-bookworm instead of Node 20 for the Docker build stage and
  `npm ci` for lockfile-based reproducibility.

## Intentionally retained dependencies

| Dependency | Constraint / follow-up |
| --- | --- |
| CDK / cdk-sqlite 0.16 | 0.18.2 exists, but requires a separate wallet/API/schema migration and recovery testing. Do not combine this with the unresolved M5 spent-proof incident. |
| SQLx 0.7.4 and local sqlx-sqlite patch | A trial SQLx 0.9 update fails Cargo resolution: its SQLite bindings require libsqlite3-sys >=0.30.1, whereas CDK 0.16 requires rusqlite 0.31 / libsqlite3-sys 0.28. Cargo permits only one SQLite native link. Upgrade together with CDK, then remove the obsolete patch. |
| Electrum client 0.18 | Keep aligned with BDK 0.30's Bitcoin 0.30 / Electrum types. Moving to the current split BDK wallet architecture is a separate migration. |
| Reqwest 0.12, rand 0.8, prometheus-client 0.22, base64 0.22, sha2 0.10 | Compatible lockfile releases refreshed; next API-breaking release lines remain separate changes. |
| TypeScript 6.0 | Latest TypeScript 7 is outside typescript-eslint 8.72's declared supported range (`>=4.8.4 <6.1.0`). |

## Verification and remaining debt

- `cargo check --locked --all-targets`: passed.
- `cargo fmt --check`: passed.
- `cargo test --locked --all-targets`: passed (6 tests across on-chain wallet, descriptor generation and API route registration).
- `npm run build`: passed with TypeScript 6 / Vite 8.3.
- `npm audit`: zero reported vulnerabilities after the update. This is **not**
  a claim about Rust dependencies; cargo-audit is not installed on this host.
- Synthetic Cashu A token metadata decoding and empty-input checks: passed,
  without contacting a mint or using real proofs.
- Chromium browser smoke: Day/Night switching and persistence, 390px landing
  layout, operator/support rendering pass without page errors. API calls were
  intercepted locally; no live backend was used.
- Frontend lint is **not clean**. An isolated copy of the committed baseline
  reports 5 errors / 8 warnings; updated tooling reports 13 errors / 8 warnings.
  The hooks checker detects 8 additional existing `set-state-in-effect`
  patterns. Baseline errors also include hook purity and an explicit `any`.
  No rule suppressions or severity reductions were added. Refactor these UI
  effects separately with session-resume and runtime-limit behavior tests.
- SQLx 0.7.4 still emits a Rust future-incompatibility warning, covered by the
  coordinated SQLx/CDK follow-up above.

Before production rollout, perform the normal staging swap smoke tests and
separately review the wallet/dependency compatibility constraints above.
