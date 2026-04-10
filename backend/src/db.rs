use crate::operations::{OPERATION_MODE_KEY, OperationMode};
use anyhow::Result as AnyResult;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{
    Error, PgPool, Postgres, Row, Transaction,
    error::BoxDynError,
    postgres::{PgPoolOptions, PgRow},
};
use std::str::FromStr;

const DEPOSIT_SELECT_FIELDS: &str = "id, amount_sats, state, address, target_confirmations, delivery_hint, metadata, txid, confirmations, last_checked_at, created_at, updated_at, minted_token, minted_amount_sats, token_ready_at, mint_attempt_count, last_mint_attempt_at, mint_error, delivery_attempt_count, last_delivery_attempt_at, delivery_error, pickup_token, pickup_revealed_at, session_id, transaction_counted_at";
const TRANSACTION_COUNTER_KEY: &str = "transaction_counter";

#[derive(Clone)]
pub struct Database {
    pub(crate) pool: PgPool,
}

impl Database {
    pub async fn connect(url: &str) -> AnyResult<Self> {
        let pool = PgPoolOptions::new().max_connections(5).connect(url).await?;
        sqlx::migrate!("./migrations").run(&pool).await?;
        Ok(Self { pool })
    }

    pub async fn load_operation_mode(&self) -> Result<OperationMode, Error> {
        let raw = sqlx::query_scalar::<_, String>("SELECT value FROM app_settings WHERE key = $1")
            .bind(OPERATION_MODE_KEY)
            .fetch_optional(&self.pool)
            .await?;

        if let Some(value) = raw {
            Ok(OperationMode::from_str(value.trim()).unwrap_or(OperationMode::Normal))
        } else {
            Ok(OperationMode::Normal)
        }
    }

    pub async fn persist_operation_mode(&self, mode: OperationMode) -> Result<(), Error> {
        sqlx::query(
            r#"
            INSERT INTO app_settings (key, value)
            VALUES ($1, $2)
            ON CONFLICT (key)
            DO UPDATE SET value = EXCLUDED.value, updated_at = NOW()
            "#,
        )
        .bind(OPERATION_MODE_KEY)
        .bind(mode.as_str())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn increment_transaction_counter(&self) -> Result<i64, Error> {
        let row = sqlx::query(
            r#"
            INSERT INTO app_settings (key, value)
            VALUES ($1, '1')
            ON CONFLICT (key)
            DO UPDATE
            SET value = ((app_settings.value::bigint + 1)::text),
                updated_at = NOW()
            RETURNING value
            "#,
        )
        .bind(TRANSACTION_COUNTER_KEY)
        .fetch_one(&self.pool)
        .await?;

        let value: String = row.try_get("value")?;
        value.parse::<i64>().map_err(|err| {
            Error::Decode(BoxDynError::from(format!(
                "invalid transaction counter value: {err}"
            )))
        })
    }

    pub async fn transaction_counter(&self) -> Result<i64, Error> {
        let value =
            sqlx::query_scalar::<_, String>("SELECT value FROM app_settings WHERE key = $1")
                .bind(TRANSACTION_COUNTER_KEY)
                .fetch_optional(&self.pool)
                .await?;

        match value {
            Some(raw) => raw.trim().parse::<i64>().map_err(|err| {
                Error::Decode(BoxDynError::from(format!(
                    "invalid transaction counter value: {err}"
                )))
            }),
            None => Ok(0),
        }
    }

    pub async fn insert_deposit(&self, deposit: &Deposit) -> Result<(), Error> {
        sqlx::query(
            r#"INSERT INTO deposits
            (id, amount_sats, state, address, target_confirmations, delivery_hint, metadata, txid, confirmations, last_checked_at, created_at, updated_at, pickup_token, pickup_revealed_at, session_id)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15)"#,
        )
        .bind(&deposit.id)
        .bind(deposit.amount_sats as i64)
        .bind(deposit.state.as_str())
        .bind(&deposit.address)
        .bind(deposit.target_confirmations as i32)
        .bind(&deposit.delivery_hint)
        .bind(deposit.metadata.as_ref().map(|v| v.to_string()))
        .bind(&deposit.txid)
        .bind(deposit.confirmations as i32)
        .bind(deposit.last_checked_at.map(|ts| ts.to_rfc3339()))
        .bind(deposit.created_at.to_rfc3339())
        .bind(deposit.updated_at.to_rfc3339())
        .bind(&deposit.pickup_token)
        .bind(deposit.pickup_revealed_at.map(|ts| ts.to_rfc3339()))
        .bind(deposit.session_id.as_deref())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn fetch_deposit(&self, id: &str) -> Result<Deposit, Error> {
        let sql = format!(
            "SELECT {} FROM deposits WHERE id = $1",
            DEPOSIT_SELECT_FIELDS
        );
        let row = sqlx::query(&sql).bind(id).fetch_one(&self.pool).await?;

        Self::map_deposit(row)
    }

    fn map_deposit(row: PgRow) -> Result<Deposit, Error> {
        let state: String = row.try_get("state")?;
        let metadata: Option<String> = row.try_get("metadata")?;
        let created_at = parse_timestamp(row.try_get("created_at")?, "created_at")?;
        let updated_at = parse_timestamp(row.try_get("updated_at")?, "updated_at")?;
        let last_checked_at = decode_optional_timestamp(&row, "last_checked_at")?;
        let token_ready_at = decode_optional_timestamp(&row, "token_ready_at")?;
        let last_mint_attempt_at = decode_optional_timestamp(&row, "last_mint_attempt_at")?;
        let last_delivery_attempt_at = decode_optional_timestamp(&row, "last_delivery_attempt_at")?;
        let pickup_revealed_at = decode_optional_timestamp(&row, "pickup_revealed_at")?;

        let parsed_state = DepositState::try_from(state.as_str())
            .map_err(|_| Error::Decode(BoxDynError::from("invalid deposit state")))?;
        let minted_token = decode_optional_string(&row, "minted_token")?;

        Ok(Deposit {
            id: row.try_get("id")?,
            amount_sats: row.try_get::<i64, _>("amount_sats")? as u64,
            state: parsed_state,
            address: row.try_get("address")?,
            target_confirmations: row.try_get::<i32, _>("target_confirmations")? as u8,
            delivery_hint: row.try_get("delivery_hint")?,
            metadata: metadata
                .as_deref()
                .map(|raw| serde_json::from_str(raw).unwrap_or(Value::Null)),
            txid: decode_optional_string(&row, "txid")?,
            confirmations: row.try_get::<i32, _>("confirmations")? as u32,
            last_checked_at,
            created_at,
            updated_at,
            minted_token: minted_token.clone(),
            session_id: decode_optional_string(&row, "session_id")?,
            pickup_token: row.try_get("pickup_token")?,
            pickup_revealed_at,
            token: match parsed_state {
                DepositState::Fulfilled => minted_token,
                _ => None,
            },
            minted_amount_sats: decode_optional_i64(&row, "minted_amount_sats")?.map(|v| v as u64),
            token_ready_at,
            mint_attempt_count: row.try_get::<i32, _>("mint_attempt_count")? as u32,
            last_mint_attempt_at,
            mint_error: decode_optional_string(&row, "mint_error")?,
            delivery_attempt_count: row.try_get::<i32, _>("delivery_attempt_count")? as u32,
            last_delivery_attempt_at,
            delivery_error: decode_optional_string(&row, "delivery_error")?,
        })
    }

    pub async fn insert_withdrawal(&self, withdrawal: &Withdrawal) -> Result<(), Error> {
        sqlx::query(
            r#"INSERT INTO withdrawals
            (id, state, delivery_address, max_fee_sats, requested_amount_sats, token_value_sats, token, txid, error, last_attempt_at, attempt_count, created_at, updated_at, payment_request_id, payment_request_creq, payment_request_expires_at, payment_request_fulfilled_at, session_id)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18)"#,
        )
        .bind(&withdrawal.id)
        .bind(withdrawal.state.as_str())
        .bind(&withdrawal.delivery_address)
        .bind(withdrawal.max_fee_sats.map(|v| v as i64))
        .bind(withdrawal.requested_amount_sats.map(|v| v as i64))
        .bind(withdrawal.token_value_sats.map(|v| v as i64))
        .bind(withdrawal.token.as_deref())
        .bind(&withdrawal.txid)
        .bind(&withdrawal.error)
        .bind(withdrawal.last_attempt_at.map(|ts| ts.to_rfc3339()))
        .bind(withdrawal.attempt_count as i32)
        .bind(withdrawal.created_at.to_rfc3339())
        .bind(withdrawal.updated_at.to_rfc3339())
        .bind(&withdrawal.payment_request_id)
        .bind(&withdrawal.payment_request_creq)
        .bind(
            withdrawal
                .payment_request_expires_at
                .map(|ts| ts.to_rfc3339()),
        )
        .bind(
            withdrawal
                .payment_request_fulfilled_at
                .map(|ts| ts.to_rfc3339()),
        )
        .bind(withdrawal.session_id.as_deref())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn fetch_withdrawal(&self, id: &str) -> Result<Withdrawal, Error> {
        let row = sqlx::query(
            r#"SELECT id, state, delivery_address, max_fee_sats, requested_amount_sats, token_value_sats, token, txid, error, last_attempt_at, attempt_count, created_at, updated_at, token_consumed, swap_fee_sats, payment_request_id, payment_request_creq, payment_request_expires_at, payment_request_fulfilled_at, session_id
            FROM withdrawals WHERE id = $1"#,
        )
        .bind(id)
        .fetch_one(&self.pool)
        .await?;

        Self::map_withdrawal(row)
    }

    fn map_withdrawal(row: PgRow) -> Result<Withdrawal, Error> {
        let state: String = row.try_get("state")?;
        let created_at = parse_timestamp(row.try_get("created_at")?, "created_at")?;
        let updated_at = parse_timestamp(row.try_get("updated_at")?, "updated_at")?;
        let last_attempt_at = decode_optional_timestamp(&row, "last_attempt_at")?;

        Ok(Withdrawal {
            id: row.try_get("id")?,
            state: WithdrawalState::try_from(state.as_str())
                .map_err(|_| Error::Decode(BoxDynError::from("invalid withdrawal state")))?,
            delivery_address: row.try_get("delivery_address")?,
            max_fee_sats: decode_optional_i64(&row, "max_fee_sats")?.map(|v| v as u64),
            requested_amount_sats: decode_optional_i64(&row, "requested_amount_sats")?
                .map(|v| v as u64),
            token_value_sats: decode_optional_i64(&row, "token_value_sats")?.map(|v| v as u64),
            token: decode_optional_string(&row, "token")?,
            txid: decode_optional_string(&row, "txid")?,
            error: decode_optional_string(&row, "error")?,
            last_attempt_at,
            attempt_count: row.try_get::<i32, _>("attempt_count")? as u32,
            created_at,
            updated_at,
            token_consumed: row.try_get::<bool, _>("token_consumed")?,
            session_id: decode_optional_string(&row, "session_id")?,
            swap_fee_sats: decode_optional_i64(&row, "swap_fee_sats")?.map(|v| v as u64),
            payment_request_id: decode_optional_string(&row, "payment_request_id")?,
            payment_request_creq: decode_optional_string(&row, "payment_request_creq")?,
            payment_request_expires_at: decode_optional_timestamp(
                &row,
                "payment_request_expires_at",
            )?,
            payment_request_fulfilled_at: decode_optional_timestamp(
                &row,
                "payment_request_fulfilled_at",
            )?,
        })
    }

    pub async fn list_withdrawals_by_state(
        &self,
        states: &[WithdrawalState],
    ) -> Result<Vec<Withdrawal>, Error> {
        if states.is_empty() {
            return Ok(Vec::new());
        }

        let placeholders = (1..=states.len())
            .map(|i| format!("${}", i))
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            r#"SELECT id, state, delivery_address, max_fee_sats, requested_amount_sats, token_value_sats, token, txid, error, last_attempt_at, attempt_count, created_at, updated_at, token_consumed, swap_fee_sats, payment_request_id, payment_request_creq, payment_request_expires_at, payment_request_fulfilled_at, session_id
            FROM withdrawals WHERE state IN ({}) ORDER BY created_at ASC"#,
            placeholders
        );

        let mut query = sqlx::query(&sql);
        for state in states {
            query = query.bind(state.as_str());
        }

        let rows = query.fetch_all(&self.pool).await?;
        rows.into_iter().map(Self::map_withdrawal).collect()
    }

    pub async fn update_withdrawal_chain_state(
        &self,
        id: &str,
        next_state: WithdrawalState,
    ) -> Result<(), Error> {
        let now = Utc::now().to_rfc3339();
        let result = sqlx::query(
            r#"UPDATE withdrawals
            SET state = $2,
                updated_at = $3
            WHERE id = $1"#,
        )
        .bind(id)
        .bind(next_state.as_str())
        .bind(&now)
        .execute(&self.pool)
        .await?;

        if result.rows_affected() == 0 {
            return Err(sqlx::Error::RowNotFound);
        }
        Ok(())
    }

    pub async fn manual_update_withdrawal_state(
        &self,
        id: &str,
        next_state: WithdrawalState,
        txid: Option<String>,
        error: Option<String>,
        reset_attempts: bool,
    ) -> Result<(), Error> {
        let now = Utc::now().to_rfc3339();
        let result = sqlx::query(
            r#"UPDATE withdrawals
            SET state = $2,
                txid = $3,
                error = $4,
                last_attempt_at = $5,
                updated_at = $5,
                attempt_count = CASE WHEN $6 THEN 0 ELSE attempt_count END
            WHERE id = $1"#,
        )
        .bind(id)
        .bind(next_state.as_str())
        .bind(txid.as_deref())
        .bind(error.as_deref())
        .bind(&now)
        .bind(reset_attempts)
        .execute(&self.pool)
        .await?;

        if result.rows_affected() == 0 {
            return Err(sqlx::Error::RowNotFound);
        }
        Ok(())
    }

    pub async fn record_withdrawal_attempt(
        &self,
        id: &str,
        next_state: WithdrawalState,
        token_value_sats: Option<u64>,
        txid: Option<&str>,
        error: Option<String>,
    ) -> Result<(), Error> {
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            r#"UPDATE withdrawals
            SET state = $1,
                token_value_sats = COALESCE($2, token_value_sats),
                txid = COALESCE($3, txid),
                error = $4,
                last_attempt_at = $5,
                attempt_count = attempt_count + 1,
                updated_at = $6
            WHERE id = $7"#,
        )
        .bind(next_state.as_str())
        .bind(token_value_sats.map(|v| v as i64))
        .bind(txid)
        .bind(error.as_deref())
        .bind(&now)
        .bind(&now)
        .bind(id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn record_token_consumed(
        &self,
        id: &str,
        amount_sats: u64,
        swap_fee_sats: Option<u64>,
    ) -> Result<(), Error> {
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            r#"UPDATE withdrawals
            SET token_consumed = TRUE,
                token_value_sats = $2,
                swap_fee_sats = $3,
                updated_at = $4
            WHERE id = $1"#,
        )
        .bind(id)
        .bind(amount_sats as i64)
        .bind(swap_fee_sats.map(|v| v as i64))
        .bind(&now)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn mark_withdrawal_transaction_counted(
        &self,
        withdrawal_id: &str,
    ) -> Result<bool, Error> {
        let now = Utc::now();
        let result = sqlx::query(
            r#"UPDATE withdrawals
                SET transaction_counted_at = $2
              WHERE id = $1 AND transaction_counted_at IS NULL"#,
        )
        .bind(withdrawal_id)
        .bind(now)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    pub async fn record_payment_request_token(&self, id: &str, token: &str) -> Result<(), Error> {
        let now = Utc::now().to_rfc3339();
        let result = sqlx::query(
            r#"UPDATE withdrawals
            SET token = $2,
                state = 'queued',
                payment_request_fulfilled_at = $3,
                updated_at = $3
            WHERE id = $1 AND state = 'funding'"#,
        )
        .bind(id)
        .bind(token)
        .bind(&now)
        .execute(&self.pool)
        .await?;

        if result.rows_affected() == 0 {
            return Err(sqlx::Error::RowNotFound);
        }
        Ok(())
    }

    pub async fn count_available_addresses(&self) -> Result<i64, Error> {
        let row = sqlx::query("SELECT COUNT(*) AS count FROM addresses WHERE state = 'available'")
            .fetch_one(&self.pool)
            .await?;
        Ok(row.try_get::<i64, _>("count")?)
    }

    pub async fn max_derivation_index(&self) -> Result<Option<i32>, Error> {
        let value: i32 =
            sqlx::query_scalar("SELECT COALESCE(MAX(derivation_index), -1) FROM addresses")
                .fetch_one(&self.pool)
                .await?;
        if value >= 0 {
            Ok(Some(value))
        } else {
            Ok(None)
        }
    }

    pub async fn insert_address(&self, address: &NewAddress) -> Result<bool, Error> {
        let result = sqlx::query(
            r#"INSERT INTO addresses
            (id, derivation_index, address, state, deposit_id, first_seen_txid, confirmations, last_checked_at, created_at, updated_at)
            VALUES ($1, $2, $3, $4, NULL, NULL, 0, NULL, $5, $6)
            ON CONFLICT(address) DO NOTHING"#,
        )
        .bind(&address.id)
        .bind(address.derivation_index as i32)
        .bind(&address.address)
        .bind(address.state.as_str())
        .bind(address.created_at.to_rfc3339())
        .bind(address.updated_at.to_rfc3339())
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    pub async fn fetch_available_address(&self) -> Result<Option<PoolAddress>, Error> {
        let row = sqlx::query(
            r#"SELECT id, address, derivation_index
            FROM addresses
            WHERE state = 'available'
            ORDER BY derivation_index ASC
            LIMIT 1"#,
        )
        .fetch_optional(&self.pool)
        .await?;

        if let Some(r) = row {
            Ok(Some(PoolAddress {
                id: r.try_get("id")?,
                address: r.try_get("address")?,
                derivation_index: r.try_get::<i32, _>("derivation_index")? as u32,
            }))
        } else {
            Ok(None)
        }
    }

    pub async fn claim_address(&self, address_id: &str, deposit_id: &str) -> Result<bool, Error> {
        let now = Utc::now().to_rfc3339();
        let result = sqlx::query(
            r#"UPDATE addresses
            SET state = 'allocated', deposit_id = $1, updated_at = $2
            WHERE id = $3 AND state = 'available'"#,
        )
        .bind(deposit_id)
        .bind(&now)
        .bind(address_id)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    pub async fn list_open_deposits(&self) -> Result<Vec<Deposit>, Error> {
        let sql = format!(
            "SELECT {} FROM deposits WHERE state IN ('pending', 'confirming')",
            DEPOSIT_SELECT_FIELDS
        );
        let rows = sqlx::query(&sql).fetch_all(&self.pool).await?;

        rows.into_iter().map(Self::map_deposit).collect()
    }

    pub async fn update_deposit_chain_state(
        &self,
        deposit_id: &str,
        txid: &str,
        confirmations: u32,
        state: DepositState,
    ) -> Result<(), Error> {
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            r#"UPDATE deposits
            SET txid = COALESCE(txid, $1),
                confirmations = $2,
                state = $3,
                last_checked_at = $4,
                updated_at = $5
            WHERE id = $6"#,
        )
        .bind(txid)
        .bind(confirmations as i32)
        .bind(state.as_str())
        .bind(&now)
        .bind(&now)
        .bind(deposit_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn update_address_observation(
        &self,
        deposit_id: &str,
        txid: &str,
        confirmations: u32,
    ) -> Result<(), Error> {
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            r#"UPDATE addresses
            SET first_seen_txid = COALESCE(first_seen_txid, $1),
                confirmations = $2,
                last_checked_at = $3,
                updated_at = $4
            WHERE deposit_id = $5"#,
        )
        .bind(txid)
        .bind(confirmations as i32)
        .bind(&now)
        .bind(&now)
        .bind(deposit_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn list_deposits_by_state(
        &self,
        states: &[DepositState],
    ) -> Result<Vec<Deposit>, Error> {
        if states.is_empty() {
            return Ok(Vec::new());
        }

        let placeholders = (1..=states.len())
            .map(|i| format!("${}", i))
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT {} FROM deposits WHERE state IN ({}) ORDER BY created_at ASC",
            DEPOSIT_SELECT_FIELDS, placeholders
        );

        let mut query = sqlx::query(&sql);
        for state in states {
            query = query.bind(state.as_str());
        }

        let rows = query.fetch_all(&self.pool).await?;
        rows.into_iter().map(Self::map_deposit).collect()
    }

    pub async fn manual_update_deposit_state(
        &self,
        id: &str,
        next_state: DepositState,
        note: Option<String>,
        clear_errors: bool,
    ) -> Result<(), Error> {
        let now = Utc::now().to_rfc3339();
        let result = sqlx::query(
            r#"UPDATE deposits
            SET state = $2,
                updated_at = $3,
                delivery_error = CASE
                    WHEN $5 THEN NULL
                    WHEN $4 IS NOT NULL THEN $4
                    ELSE delivery_error
                END,
                mint_error = CASE
                    WHEN $5 THEN NULL
                    WHEN $4 IS NOT NULL THEN $4
                    ELSE mint_error
                END
            WHERE id = $1"#,
        )
        .bind(id)
        .bind(next_state.as_str())
        .bind(&now)
        .bind(note.as_deref())
        .bind(clear_errors)
        .execute(&self.pool)
        .await?;

        if result.rows_affected() == 0 {
            return Err(sqlx::Error::RowNotFound);
        }
        Ok(())
    }

    pub async fn record_mint_success(
        &self,
        deposit_id: &str,
        token: &str,
        amount_sats: u64,
        next_state: DepositState,
    ) -> Result<(), Error> {
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            r#"UPDATE deposits
            SET state = $1,
                minted_token = $2,
                minted_amount_sats = $3,
                mint_attempt_count = mint_attempt_count + 1,
                last_mint_attempt_at = $4,
                mint_error = NULL,
                updated_at = $4
            WHERE id = $5"#,
        )
        .bind(next_state.as_str())
        .bind(token)
        .bind(amount_sats as i64)
        .bind(&now)
        .bind(deposit_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn record_mint_failure(
        &self,
        deposit_id: &str,
        next_state: DepositState,
        error: &str,
    ) -> Result<(), Error> {
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            r#"UPDATE deposits
            SET state = $1,
                mint_attempt_count = mint_attempt_count + 1,
                last_mint_attempt_at = $2,
                mint_error = $3,
                updated_at = $2
            WHERE id = $4"#,
        )
        .bind(next_state.as_str())
        .bind(&now)
        .bind(error)
        .bind(deposit_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn record_delivery_success(
        &self,
        deposit_id: &str,
        next_state: DepositState,
    ) -> Result<(), Error> {
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            r#"UPDATE deposits
            SET state = $1,
                token_ready_at = COALESCE(token_ready_at, $2),
                delivery_attempt_count = delivery_attempt_count + 1,
                last_delivery_attempt_at = $2,
                delivery_error = NULL,
                updated_at = $2
            WHERE id = $3"#,
        )
        .bind(next_state.as_str())
        .bind(&now)
        .bind(deposit_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn record_delivery_failure(
        &self,
        deposit_id: &str,
        next_state: DepositState,
        error: &str,
    ) -> Result<(), Error> {
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            r#"UPDATE deposits
            SET state = $1,
                delivery_attempt_count = delivery_attempt_count + 1,
                last_delivery_attempt_at = $2,
                delivery_error = $3,
                updated_at = $2
            WHERE id = $4"#,
        )
        .bind(next_state.as_str())
        .bind(&now)
        .bind(error)
        .bind(deposit_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn record_pickup_success(&self, deposit_id: &str) -> Result<(), Error> {
        let now = Utc::now().to_rfc3339();
        let result = sqlx::query(
            r#"UPDATE deposits
            SET state = $2,
                pickup_revealed_at = $3,
                minted_token = NULL,
                pickup_token = CONCAT('claimed-', id),
                updated_at = $3
            WHERE id = $1"#,
        )
        .bind(deposit_id)
        .bind(DepositState::Fulfilled.as_str())
        .bind(&now)
        .execute(&self.pool)
        .await?;

        if result.rows_affected() == 0 {
            return Err(sqlx::Error::RowNotFound);
        }

        Ok(())
    }

    pub async fn mark_deposit_transaction_counted(&self, deposit_id: &str) -> Result<bool, Error> {
        let now = Utc::now();
        let result = sqlx::query(
            r#"UPDATE deposits
                SET transaction_counted_at = $2
              WHERE id = $1 AND transaction_counted_at IS NULL"#,
        )
        .bind(deposit_id)
        .bind(now)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() == 1)
    }

    /// Atomically claims the minted ecash for a deposit exactly once.
    ///
    /// This provides a durable idempotency boundary against replay: once claimed,
    /// the minted token is removed from `deposits`, a claim row is inserted, and
    /// the pickup token is rotated so replays fail validation.
    pub async fn claim_deposit_pickup(
        &self,
        deposit_id: &str,
        pickup_token: &str,
    ) -> Result<String, Error> {
        let mut tx: Transaction<'_, Postgres> = self.pool.begin().await?;

        // Lock the row so concurrent claim attempts serialize.
        let row = sqlx::query(
            r#"SELECT state, minted_token
               FROM deposits
              WHERE id = $1 AND pickup_token = $2
              FOR UPDATE"#,
        )
        .bind(deposit_id)
        .bind(pickup_token)
        .fetch_one(&mut *tx)
        .await?;

        let state: String = row.try_get("state")?;
        let minted_token: Option<String> = row.try_get("minted_token")?;

        let state = state.trim();

        // If it was already fulfilled, we never re-serve the token.
        if state == DepositState::Fulfilled.as_str() {
            tx.rollback().await?;
            return Err(sqlx::Error::RowNotFound);
        }

        let minted = minted_token.ok_or_else(|| sqlx::Error::RowNotFound)?;
        if state != DepositState::Ready.as_str() {
            tx.rollback().await?;
            return Err(sqlx::Error::RowNotFound);
        }

        // Insert the claim row (primary key on deposit_id makes this one-shot).
        // If this conflicts due to a race, we treat it as already claimed.
        let insert = sqlx::query(
            r#"INSERT INTO deposit_claims (deposit_id, minted_token)
               VALUES ($1, $2)
               ON CONFLICT (deposit_id) DO NOTHING"#,
        )
        .bind(deposit_id)
        .bind(&minted)
        .execute(&mut *tx)
        .await?;

        if insert.rows_affected() == 0 {
            tx.rollback().await?;
            return Err(sqlx::Error::RowNotFound);
        }

        let now = Utc::now().to_rfc3339();
        sqlx::query(
            r#"UPDATE deposits
                SET state = $2,
                    pickup_revealed_at = $3,
                    minted_token = NULL,
                    pickup_token = CONCAT('claimed-', id),
                    updated_at = $3
              WHERE id = $1"#,
        )
        .bind(deposit_id)
        .bind(DepositState::Fulfilled.as_str())
        .bind(&now)
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;
        Ok(minted)
    }

    pub async fn ledger_deposit_liabilities(&self) -> Result<Vec<StateLiabilityRow>, Error> {
        let rows = sqlx::query(
            r#"SELECT state, deposit_count, total_sats FROM ledger_deposit_liabilities"#,
        )
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter()
            .map(|row| {
                Ok(StateLiabilityRow {
                    state: row.try_get("state")?,
                    count: row.try_get::<i64, _>("deposit_count")? as u64,
                    amount_sats: row.try_get::<i64, _>("total_sats")? as u64,
                })
            })
            .collect()
    }

    pub async fn ledger_withdrawal_liabilities(&self) -> Result<Vec<StateLiabilityRow>, Error> {
        let rows = sqlx::query(
            r#"SELECT state, withdrawal_count, total_sats FROM ledger_withdrawal_liabilities"#,
        )
        .fetch_all(&self.pool)
        .await?;

        rows.into_iter()
            .map(|row| {
                Ok(StateLiabilityRow {
                    state: row.try_get("state")?,
                    count: row.try_get::<i64, _>("withdrawal_count")? as u64,
                    amount_sats: row.try_get::<i64, _>("total_sats")? as u64,
                })
            })
            .collect()
    }

    pub async fn reserved_onchain_withdrawal_sats(&self) -> Result<u64, Error> {
        let total = sqlx::query_scalar::<_, i64>(
            r#"SELECT COALESCE(SUM(COALESCE(token_value_sats, requested_amount_sats, 0)), 0)
               FROM withdrawals
               WHERE state IN ('queued', 'broadcasting', 'confirming')"#,
        )
        .fetch_one(&self.pool)
        .await?;

        Ok(total.max(0) as u64)
    }

    pub async fn reserved_cashu_deposit_sats(&self) -> Result<u64, Error> {
        let total = sqlx::query_scalar::<_, i64>(
            r#"SELECT COALESCE(SUM(amount_sats), 0)
               FROM deposits
               WHERE state IN ('confirming', 'minting', 'delivering', 'ready')"#,
        )
        .fetch_one(&self.pool)
        .await?;

        Ok(total.max(0) as u64)
    }

    pub async fn create_session(&self, session: &Session) -> Result<(), Error> {
        sqlx::query(
            r#"INSERT INTO sessions (id, token_hash, created_at, last_seen_at, expires_at) VALUES ($1, $2, $3, $4, $5)"#,
        )
        .bind(&session.id)
        .bind(&session.token_hash)
        .bind(session.created_at.to_rfc3339())
        .bind(session.last_seen_at.to_rfc3339())
        .bind(session.expires_at.to_rfc3339())
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn fetch_session_by_token_hash(&self, token_hash: &str) -> Result<Session, Error> {
        let row = sqlx::query(
            r#"SELECT id, token_hash, created_at, last_seen_at, expires_at FROM sessions WHERE token_hash = $1"#,
        )
        .bind(token_hash)
        .fetch_one(&self.pool)
        .await?;

        Self::map_session(row)
    }

    pub async fn touch_session(
        &self,
        id: &str,
        last_seen_at: DateTime<Utc>,
        expires_at: DateTime<Utc>,
    ) -> Result<(), Error> {
        sqlx::query(r#"UPDATE sessions SET last_seen_at = $2, expires_at = $3 WHERE id = $1"#)
            .bind(id)
            .bind(last_seen_at.to_rfc3339())
            .bind(expires_at.to_rfc3339())
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn delete_expired_sessions(&self, now: DateTime<Utc>) -> Result<u64, Error> {
        let result = sqlx::query(r#"DELETE FROM sessions WHERE expires_at <= $1"#)
            .bind(now.to_rfc3339())
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected())
    }

    fn map_session(row: PgRow) -> Result<Session, Error> {
        Ok(Session {
            id: row.try_get("id")?,
            token_hash: row.try_get("token_hash")?,
            created_at: parse_timestamp(row.try_get("created_at")?, "created_at")?,
            last_seen_at: parse_timestamp(row.try_get("last_seen_at")?, "last_seen_at")?,
            expires_at: parse_timestamp(row.try_get("expires_at")?, "expires_at")?,
        })
    }
}

#[derive(Debug, Clone)]
pub struct StateLiabilityRow {
    pub state: String,
    pub count: u64,
    pub amount_sats: u64,
}

#[derive(Clone, Debug)]
pub struct Session {
    pub id: String,
    pub token_hash: String,
    pub created_at: DateTime<Utc>,
    pub last_seen_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DepositState {
    Pending,
    Confirming,
    Minting,
    Delivering,
    Ready,
    Fulfilled,
    Failed,
    ArchivedByOperator,
}

impl DepositState {
    pub fn as_str(&self) -> &'static str {
        match self {
            DepositState::Pending => "pending",
            DepositState::Confirming => "confirming",
            DepositState::Minting => "minting",
            DepositState::Delivering => "delivering",
            DepositState::Ready => "ready",
            DepositState::Fulfilled => "fulfilled",
            DepositState::Failed => "failed",
            DepositState::ArchivedByOperator => "archived_by_operator",
        }
    }
}

impl TryFrom<&str> for DepositState {
    type Error = &'static str;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            "pending" => Ok(DepositState::Pending),
            "confirming" => Ok(DepositState::Confirming),
            "minting" => Ok(DepositState::Minting),
            "delivering" => Ok(DepositState::Delivering),
            "ready" => Ok(DepositState::Ready),
            "fulfilled" => Ok(DepositState::Fulfilled),
            "failed" => Ok(DepositState::Failed),
            "archived_by_operator" => Ok(DepositState::ArchivedByOperator),
            _ => Err("unknown deposit state"),
        }
    }
}

#[derive(Serialize, Clone, Debug)]
pub struct Deposit {
    pub id: String,
    pub amount_sats: u64,
    pub state: DepositState,
    pub address: String,
    pub target_confirmations: u8,
    pub delivery_hint: Option<String>,
    pub metadata: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub txid: Option<String>,
    pub confirmations: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_checked_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(skip_serializing)]
    pub minted_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(skip_serializing)]
    pub pickup_token: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pickup_revealed_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub minted_amount_sats: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token_ready_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "is_zero_u32")]
    pub mint_attempt_count: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_mint_attempt_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mint_error: Option<String>,
    #[serde(skip_serializing_if = "is_zero_u32")]
    pub delivery_attempt_count: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_delivery_attempt_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delivery_error: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WithdrawalState {
    Funding,
    Queued,
    Broadcasting,
    Confirming,
    Settled,
    Failed,
    ArchivedByOperator,
}

impl WithdrawalState {
    pub fn as_str(&self) -> &'static str {
        match self {
            WithdrawalState::Funding => "funding",
            WithdrawalState::Queued => "queued",
            WithdrawalState::Broadcasting => "broadcasting",
            WithdrawalState::Confirming => "confirming",
            WithdrawalState::Settled => "settled",
            WithdrawalState::Failed => "failed",
            WithdrawalState::ArchivedByOperator => "archived_by_operator",
        }
    }
}

impl TryFrom<&str> for WithdrawalState {
    type Error = &'static str;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        match value {
            "funding" => Ok(WithdrawalState::Funding),
            "queued" => Ok(WithdrawalState::Queued),
            "broadcasting" => Ok(WithdrawalState::Broadcasting),
            "confirming" => Ok(WithdrawalState::Confirming),
            "settled" => Ok(WithdrawalState::Settled),
            "failed" => Ok(WithdrawalState::Failed),
            "archived_by_operator" => Ok(WithdrawalState::ArchivedByOperator),
            _ => Err("unknown withdrawal state"),
        }
    }
}

#[derive(Serialize, Clone, Debug)]
pub struct Withdrawal {
    pub id: String,
    pub state: WithdrawalState,
    pub delivery_address: String,
    pub max_fee_sats: Option<u64>,
    pub requested_amount_sats: Option<u64>,
    pub token_value_sats: Option<u64>,
    #[serde(skip_serializing)]
    pub token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub txid: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_attempt_at: Option<DateTime<Utc>>,
    pub attempt_count: u32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub token_consumed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub swap_fee_sats: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payment_request_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payment_request_creq: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payment_request_expires_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payment_request_fulfilled_at: Option<DateTime<Utc>>,
}

#[derive(Clone, Copy, Debug)]
pub enum AddressState {
    Available,
    Allocated,
    Retired,
}

impl AddressState {
    pub fn as_str(&self) -> &'static str {
        match self {
            AddressState::Available => "available",
            AddressState::Allocated => "allocated",
            AddressState::Retired => "retired",
        }
    }
}

#[derive(Clone, Debug)]
pub struct PoolAddress {
    pub id: String,
    pub address: String,
    pub derivation_index: u32,
}

#[derive(Clone, Debug)]
pub struct NewAddress {
    pub id: String,
    pub derivation_index: u32,
    pub address: String,
    pub state: AddressState,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

fn parse_timestamp(value: String, column: &'static str) -> Result<DateTime<Utc>, Error> {
    DateTime::parse_from_rfc3339(&value)
        .map(|dt| dt.with_timezone(&Utc))
        .map_err(|err| Error::Decode(BoxDynError::from(format!("{column}: {err}"))))
}

fn decode_optional_timestamp(
    row: &PgRow,
    column: &'static str,
) -> Result<Option<DateTime<Utc>>, Error> {
    match row.try_get::<String, _>(column) {
        Ok(raw) => Ok(Some(parse_timestamp(raw, column)?)),
        Err(err) if is_null_column(&err) => Ok(None),
        Err(err) => Err(err),
    }
}

fn decode_optional_string(row: &PgRow, column: &'static str) -> Result<Option<String>, Error> {
    match row.try_get::<String, _>(column) {
        Ok(value) => Ok(Some(value)),
        Err(err) if is_null_column(&err) => Ok(None),
        Err(err) => Err(err),
    }
}

fn decode_optional_i64(row: &PgRow, column: &'static str) -> Result<Option<i64>, Error> {
    match row.try_get::<i64, _>(column) {
        Ok(value) => Ok(Some(value)),
        Err(err) if is_null_column(&err) => Ok(None),
        Err(err) => Err(err),
    }
}

fn is_zero_u32(value: &u32) -> bool {
    *value == 0
}

fn is_null_column(err: &Error) -> bool {
    matches!(err, Error::ColumnDecode { source, .. } if source
        .to_string()
        .to_ascii_lowercase()
        .contains("null"))
}
