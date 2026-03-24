-- Refresh ledger views to exclude operator-archived rows

DROP VIEW IF EXISTS ledger_deposit_liabilities;
CREATE VIEW ledger_deposit_liabilities AS
SELECT
    state,
    COUNT(*)::BIGINT AS deposit_count,
    COALESCE(SUM(amount_sats), 0)::BIGINT AS total_sats
FROM deposits
WHERE state NOT IN ('failed', 'fulfilled', 'archived_by_operator')
GROUP BY state;

DROP VIEW IF EXISTS ledger_withdrawal_liabilities;
CREATE VIEW ledger_withdrawal_liabilities AS
SELECT
    state,
    COUNT(*)::BIGINT AS withdrawal_count,
    COALESCE(SUM(COALESCE(token_value_sats, requested_amount_sats, 0)), 0)::BIGINT AS total_sats
FROM withdrawals
WHERE state NOT IN ('failed', 'settled', 'archived_by_operator')
GROUP BY state;
