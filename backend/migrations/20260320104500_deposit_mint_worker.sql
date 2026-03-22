ALTER TABLE deposits
    ADD COLUMN minted_token TEXT,
    ADD COLUMN minted_amount_sats BIGINT,
    ADD COLUMN token_ready_at TEXT,
    ADD COLUMN mint_attempt_count INTEGER NOT NULL DEFAULT 0,
    ADD COLUMN last_mint_attempt_at TEXT,
    ADD COLUMN mint_error TEXT,
    ADD COLUMN delivery_attempt_count INTEGER NOT NULL DEFAULT 0,
    ADD COLUMN last_delivery_attempt_at TEXT,
    ADD COLUMN delivery_error TEXT;
