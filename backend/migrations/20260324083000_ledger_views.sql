-- Ledger & reconciliation helper views

CREATE VIEW ledger_deposit_liabilities AS
SELECT
    state,
    COUNT(*)::BIGINT AS deposit_count,
    COALESCE(SUM(amount_sats), 0)::BIGINT AS total_sats
FROM deposits
WHERE state NOT IN ('failed', 'fulfilled')
GROUP BY state;

CREATE VIEW ledger_withdrawal_liabilities AS
SELECT
    state,
    COUNT(*)::BIGINT AS withdrawal_count,
    COALESCE(SUM(COALESCE(token_value_sats, requested_amount_sats, 0)), 0)::BIGINT AS total_sats
FROM withdrawals
WHERE state NOT IN ('failed', 'settled')
GROUP BY state;

CREATE VIEW ledger_onchain_wallet_balances AS
WITH live_utxos AS (
    SELECT
        u.wallet_id,
        u.txid,
        u.value,
        t.confirmation_height
    FROM onchain_wallet_utxos u
    LEFT JOIN onchain_wallet_txs t
        ON t.wallet_id = u.wallet_id
       AND t.txid = u.txid
    WHERE u.is_spent = FALSE
)
SELECT
    w.id AS wallet_id,
    w.network,
    COALESCE(SUM(CASE WHEN l.confirmation_height IS NOT NULL THEN l.value END), 0)::BIGINT AS confirmed_sats,
    COALESCE(SUM(CASE WHEN l.confirmation_height IS NULL THEN l.value END), 0)::BIGINT AS unconfirmed_sats
FROM onchain_wallets w
LEFT JOIN live_utxos l ON l.wallet_id = w.id
GROUP BY w.id, w.network;
