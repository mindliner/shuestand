# Changelog

## 2026-03-20
- Added the shared wallet module + operator CLI (`invoice`, `send`, `receive`, `pay`) so float management no longer depends on the Nutshell/Python tooling.
- Split the backend into dedicated modules (`config`, `db`, `http`, `workers`) and rewired `main.rs` to just orchestrate config, workers, and server bootstrap.
- Introduced `src/bin/wallet.rs` and documented the new commands in the README.
