ALTER TABLE withdrawals
    ADD COLUMN token_consumed BOOLEAN NOT NULL DEFAULT FALSE,
    ADD COLUMN swap_fee_sats BIGINT;
