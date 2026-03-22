-- On-chain wallet persistence tables for BDK-backed spending wallet

CREATE TABLE IF NOT EXISTS onchain_wallets (
    id UUID PRIMARY KEY,
    spend_descriptor TEXT NOT NULL,
    change_descriptor TEXT,
    network TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE IF NOT EXISTS onchain_wallet_scripts (
    wallet_id UUID NOT NULL REFERENCES onchain_wallets(id) ON DELETE CASCADE,
    keychain SMALLINT NOT NULL,
    child_index INTEGER NOT NULL,
    script BYTEA NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (wallet_id, keychain, child_index),
    UNIQUE (wallet_id, script)
);
CREATE INDEX IF NOT EXISTS onchain_wallet_scripts_wallet_idx
    ON onchain_wallet_scripts (wallet_id);

CREATE TABLE IF NOT EXISTS onchain_wallet_utxos (
    wallet_id UUID NOT NULL REFERENCES onchain_wallets(id) ON DELETE CASCADE,
    txid BYTEA NOT NULL,
    vout INTEGER NOT NULL,
    script BYTEA NOT NULL,
    value BIGINT NOT NULL,
    keychain SMALLINT NOT NULL,
    is_spent BOOLEAN NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (wallet_id, txid, vout)
);
CREATE INDEX IF NOT EXISTS onchain_wallet_utxos_script_idx
    ON onchain_wallet_utxos (wallet_id, script);

CREATE TABLE IF NOT EXISTS onchain_wallet_raw_txs (
    wallet_id UUID NOT NULL REFERENCES onchain_wallets(id) ON DELETE CASCADE,
    txid BYTEA NOT NULL,
    transaction BYTEA NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (wallet_id, txid)
);

CREATE TABLE IF NOT EXISTS onchain_wallet_txs (
    wallet_id UUID NOT NULL REFERENCES onchain_wallets(id) ON DELETE CASCADE,
    txid BYTEA NOT NULL,
    received BIGINT NOT NULL,
    sent BIGINT NOT NULL,
    fee BIGINT,
    confirmation_height INTEGER,
    confirmation_timestamp BIGINT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (wallet_id, txid)
);

CREATE TABLE IF NOT EXISTS onchain_wallet_last_indices (
    wallet_id UUID NOT NULL REFERENCES onchain_wallets(id) ON DELETE CASCADE,
    keychain SMALLINT NOT NULL,
    value INTEGER NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (wallet_id, keychain)
);

CREATE TABLE IF NOT EXISTS onchain_wallet_sync_times (
    wallet_id UUID PRIMARY KEY REFERENCES onchain_wallets(id) ON DELETE CASCADE,
    block_height INTEGER NOT NULL,
    block_timestamp BIGINT NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE TABLE IF NOT EXISTS onchain_wallet_descriptor_checksums (
    wallet_id UUID NOT NULL REFERENCES onchain_wallets(id) ON DELETE CASCADE,
    keychain SMALLINT NOT NULL,
    checksum BYTEA NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (wallet_id, keychain)
);
