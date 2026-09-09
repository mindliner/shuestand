ALTER TABLE withdrawals
    ADD COLUMN lightning_quote_id TEXT,
    ADD COLUMN lightning_invoice_request TEXT,
    ADD COLUMN lightning_invoice_expires_at TEXT,
    ADD COLUMN lightning_invoice_paid_at TEXT;
