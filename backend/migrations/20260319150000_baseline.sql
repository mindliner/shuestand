-- Baseline schema for shuestand (Postgres)

CREATE TABLE IF NOT EXISTS deposits (
    id TEXT PRIMARY KEY,
    amount_sats BIGINT NOT NULL,
    state TEXT NOT NULL,
    address TEXT NOT NULL,
    target_confirmations INTEGER NOT NULL,
    delivery_hint TEXT,
    metadata TEXT,
    txid TEXT,
    confirmations INTEGER DEFAULT 0,
    last_checked_at TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_deposits_state ON deposits (state);

CREATE TABLE IF NOT EXISTS withdrawals (
    id TEXT PRIMARY KEY,
    state TEXT NOT NULL,
    delivery_address TEXT NOT NULL,
    max_fee_sats BIGINT,
    token_value_sats BIGINT,
    token TEXT NOT NULL,
    txid TEXT,
    error TEXT,
    last_attempt_at TEXT,
    attempt_count INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_withdrawals_state ON withdrawals (state);

CREATE TABLE IF NOT EXISTS addresses (
    id TEXT PRIMARY KEY,
    derivation_index INTEGER NOT NULL,
    address TEXT NOT NULL UNIQUE,
    state TEXT NOT NULL,
    deposit_id TEXT,
    first_seen_txid TEXT,
    confirmations INTEGER DEFAULT 0,
    last_checked_at TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_addresses_state ON addresses (state);
CREATE INDEX IF NOT EXISTS idx_addresses_deposit ON addresses (deposit_id);
