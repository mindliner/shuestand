ALTER TABLE withdrawals
    ADD COLUMN requested_amount_sats BIGINT,
    ADD COLUMN payment_request_id TEXT,
    ADD COLUMN payment_request_creq TEXT,
    ADD COLUMN payment_request_expires_at TEXT,
    ADD COLUMN payment_request_fulfilled_at TEXT;

ALTER TABLE withdrawals
    ALTER COLUMN token DROP NOT NULL;
