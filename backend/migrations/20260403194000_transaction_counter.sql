-- Track whether a deposit or withdrawal has already been counted in the
-- global transaction counter so we only emit the webhook once per entity.

ALTER TABLE deposits
    ADD COLUMN IF NOT EXISTS transaction_counted_at TIMESTAMPTZ;

ALTER TABLE withdrawals
    ADD COLUMN IF NOT EXISTS transaction_counted_at TIMESTAMPTZ;
