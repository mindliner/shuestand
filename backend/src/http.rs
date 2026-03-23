use axum::{
    Json, Router,
    extract::{Path, State},
    http::{
        HeaderMap, HeaderValue, StatusCode,
        header::{AUTHORIZATION, CONTENT_TYPE, HOST},
    },
    response::IntoResponse,
    routing::{get, post},
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{borrow::ToOwned, fs, sync::Arc, time::Instant};
use uuid::Uuid;

use crate::AppState;
use backend::cashu::{TokenMintError, token_mint_url, token_total_amount};
use backend::db::{Deposit, DepositState, Withdrawal, WithdrawalState};
use backend::onchain::{OnchainBalance, OnchainWallet};
use backend::wallet::WalletHandle;
use cdk::Amount;
use cdk::amount::SplitTarget;
use cdk::nuts::nut00::KnownMethod;
use cdk::nuts::{MintQuoteState, PaymentMethod};
use cdk::wallet::{MintQuote, SendOptions};
use urlencoding::encode;

type ApiResult<T> = Result<Json<ApiResponse<T>>, (StatusCode, Json<ApiError>)>;
const WALLET_TOPUP_LABEL: &str = "Shuestand hot wallet top-up";
const PAYMENT_REQUEST_TTL_SECS: i64 = 300;

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(healthcheck))
        .route("/metrics", get(export_metrics))
        .route("/api/v1/deposits", post(create_deposit))
        .route("/api/v1/deposits/:id", get(get_deposit))
        .route("/api/v1/withdrawals", post(request_withdrawal))
        .route("/api/v1/withdrawals/:id", get(get_withdrawal))
        .route(
            "/api/v1/withdrawals/:id/nut18",
            post(submit_payment_request),
        )
        .route("/api/v1/wallet/balance", get(get_wallet_balance))
        .route("/api/v1/wallet/send", post(send_wallet_payment))
        .route("/api/v1/wallet/sync", post(sync_wallet))
        .route("/api/v1/wallet/topup", get(get_wallet_topup))
        .route("/api/v1/cashu/invoices", post(create_cashu_invoice))
        .route("/api/v1/cashu/invoices/:id", get(get_cashu_invoice))
        .route("/api/v1/cashu/invoices/:id/mint", post(mint_cashu_invoice))
        .route(
            "/api/v1/cashu/wallet/balance",
            get(get_cashu_wallet_balance),
        )
        .route("/api/v1/cashu/send", post(send_cashu_token))
        .route("/api/v1/cashu/introspect", post(inspect_cashu_token))
        .route("/api/v1/float/status", get(get_float_status))
        .with_state(state)
}

#[derive(Serialize)]
struct ApiResponse<T> {
    data: T,
}

#[derive(Serialize)]
struct ApiError {
    code: &'static str,
    message: String,
}

#[derive(Deserialize, Debug)]
struct DepositRequest {
    amount_sats: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    delivery_hint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    metadata: Option<serde_json::Value>,
}

#[derive(Deserialize, Debug)]
struct WithdrawalRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    token: Option<String>,
    delivery_address: String,
    amount_sats: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    max_fee_sats: Option<u64>,
    #[serde(default)]
    create_payment_request: bool,
}

#[derive(Serialize)]
struct WithdrawalPaymentRequestView {
    creq: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    expires_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    fulfilled_at: Option<String>,
}

#[derive(Serialize)]
struct WithdrawalView {
    #[serde(flatten)]
    withdrawal: Withdrawal,
    #[serde(skip_serializing_if = "Option::is_none")]
    source_mint_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    is_foreign_mint: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    payment_request: Option<WithdrawalPaymentRequestView>,
}

impl WithdrawalView {
    fn new(
        withdrawal: Withdrawal,
        canonical_mint: Option<&str>,
        known_mint: Option<String>,
    ) -> Self {
        let source_mint_url = if let Some(url) = known_mint {
            Some(url)
        } else if let Some(token) = withdrawal.token.as_deref() {
            match token_mint_url(token) {
                Ok(url) => Some(url),
                Err(err) => {
                    tracing::warn!(target: "backend", error = %err, id = %withdrawal.id, "failed to parse mint for withdrawal");
                    None
                }
            }
        } else {
            None
        };
        let is_foreign_mint = match (canonical_mint, source_mint_url.as_deref()) {
            (Some(expected), Some(actual)) => Some(actual != expected),
            _ => None,
        };
        let payment_request =
            withdrawal
                .payment_request_creq
                .as_ref()
                .map(|creq| WithdrawalPaymentRequestView {
                    creq: creq.clone(),
                    expires_at: withdrawal
                        .payment_request_expires_at
                        .map(|ts| ts.to_rfc3339()),
                    fulfilled_at: withdrawal
                        .payment_request_fulfilled_at
                        .map(|ts| ts.to_rfc3339()),
                });
        Self {
            withdrawal,
            source_mint_url,
            is_foreign_mint,
            payment_request,
        }
    }
}

#[derive(Serialize)]
struct Nut18LockOption {
    #[serde(rename = "k")]
    kind: String,
    #[serde(rename = "d")]
    data: String,
    #[serde(rename = "t", skip_serializing_if = "Option::is_none")]
    tags: Option<Vec<Vec<String>>>,
}

#[derive(Serialize)]
struct Nut18Transport {
    #[serde(rename = "t")]
    kind: String,
    #[serde(rename = "a")]
    target: String,
    #[serde(rename = "g", skip_serializing_if = "Option::is_none")]
    tags: Option<Vec<Vec<String>>>,
}

#[derive(Serialize)]
struct Nut18PaymentRequest {
    #[serde(rename = "i", skip_serializing_if = "Option::is_none")]
    id: Option<String>,
    #[serde(rename = "a", skip_serializing_if = "Option::is_none")]
    amount: Option<u64>,
    #[serde(rename = "u", skip_serializing_if = "Option::is_none")]
    unit: Option<String>,
    #[serde(rename = "s", skip_serializing_if = "Option::is_none")]
    single_use: Option<bool>,
    #[serde(rename = "m", skip_serializing_if = "Option::is_none")]
    mints: Option<Vec<String>>,
    #[serde(rename = "d", skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    #[serde(rename = "t", skip_serializing_if = "Option::is_none")]
    transports: Option<Vec<Nut18Transport>>,
    #[serde(rename = "nut10", skip_serializing_if = "Option::is_none")]
    nut10: Option<Nut18LockOption>,
}

#[derive(Deserialize, Debug)]
struct WalletSendRequest {
    address: String,
    amount_sats: u64,
    fee_rate_vb: f32,
}

#[derive(Serialize)]
struct WalletBalanceResponse {
    confirmed: u64,
    trusted_pending: u64,
    untrusted_pending: u64,
    immature: u64,
}

impl From<OnchainBalance> for WalletBalanceResponse {
    fn from(value: OnchainBalance) -> Self {
        Self {
            confirmed: value.confirmed,
            trusted_pending: value.trusted_pending,
            untrusted_pending: value.untrusted_pending,
            immature: value.immature,
        }
    }
}

#[derive(Serialize)]
struct WalletSendResponse {
    txid: String,
}

#[derive(Serialize)]
struct WalletTopUpResponse {
    address: String,
    bip21_uri: String,
}

#[derive(Deserialize)]
struct CashuInvoiceRequest {
    amount_sats: u64,
    #[serde(default)]
    bolt12: bool,
}

#[derive(Serialize)]
struct CashuInvoiceStatusResponse {
    quote_id: String,
    amount_sats: u64,
    amount_paid_sats: u64,
    amount_issued_sats: u64,
    request: String,
    method: String,
    state: String,
    expires_at: u64,
}

#[derive(Serialize)]
struct CashuMintResponse {
    minted: bool,
    amount_sats: u64,
}

#[derive(Serialize)]
struct CashuWalletBalanceResponse {
    spendable: u64,
    pending: u64,
    reserved: u64,
}

#[derive(Deserialize)]
struct CashuSendRequest {
    amount_sats: u64,
}

#[derive(Serialize)]
struct CashuSendResponse {
    token: String,
}

#[derive(Deserialize, Debug)]
struct Nut18PaymentPayload {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    _memo: Option<String>,
    mint: String,
    unit: String,
    proofs: Vec<serde_json::Value>,
}

#[derive(Serialize)]
struct PaymentAcceptResponse {
    accepted: bool,
    amount_sats: u64,
}

#[derive(Deserialize)]
struct TokenInspectRequest {
    token: String,
}

#[derive(Serialize)]
struct TokenInspectResponse {
    mint_url: String,
    is_foreign: bool,
}

#[derive(Serialize)]
struct FloatStatusResponse {
    target_sats: u64,
    min_ratio: f32,
    max_ratio: f32,
    onchain: WalletFloatStatusPayload,
    cashu: WalletFloatStatusPayload,
    total_balance_sats: u64,
    drift_sats: i64,
}

#[derive(Serialize)]
struct WalletFloatStatusPayload {
    balance_sats: u64,
    ratio: f32,
    state: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    updated_at: Option<String>,
}

impl WalletFloatStatusPayload {
    fn from(status: &crate::WalletFloatStatus) -> Self {
        Self {
            balance_sats: status.balance_sats,
            ratio: status.ratio,
            state: status.state.as_str(),
            updated_at: status.updated_at.map(|ts| ts.to_rfc3339()),
        }
    }
}

async fn healthcheck() -> Json<ApiResponse<HealthResponse>> {
    Json(ApiResponse {
        data: HealthResponse { status: "ok" },
    })
}

#[derive(Serialize)]
struct HealthResponse {
    status: &'static str,
}

async fn export_metrics(State(state): State<AppState>) -> impl IntoResponse {
    let mut headers = HeaderMap::new();
    headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_static("text/plain; version=0.0.4"),
    );
    (StatusCode::OK, headers, state.metrics.encode())
}

async fn create_deposit(
    State(state): State<AppState>,
    Json(req): Json<DepositRequest>,
) -> ApiResult<Deposit> {
    const MIN_DEPOSIT_SATS: u64 = super::MIN_DEPOSIT_SATS;
    const MAX_DEPOSIT_SATS: u64 = super::MAX_DEPOSIT_SATS;

    if req.amount_sats < MIN_DEPOSIT_SATS || req.amount_sats > MAX_DEPOSIT_SATS {
        return Err(invalid_request("amount_sats is outside the allowed range"));
    }

    let cashu_wallet = state
        .cashu_wallet
        .as_ref()
        .ok_or_else(|| unavailable("cashu wallet not configured"))?;
    let spendable = {
        let guard = cashu_wallet.lock().await;
        guard.total_balance().await.map_err(server_error)?.to_u64()
    };
    let ratio = state.single_request_cap_ratio;
    let deposit_cap = ((spendable as f64) * ratio).floor() as u64;
    if deposit_cap == 0 {
        return Err(unavailable(
            "cashu float is depleted; please contact the operator",
        ));
    }
    if req.amount_sats > deposit_cap {
        tracing::warn!(
            target: "backend",
            requested = req.amount_sats,
            spendable,
            deposit_cap,
            "deposit request exceeds cashu float cap"
        );
        return Err(invalid_request(format!(
            "amount_sats exceeds the current Cashu float cap ({} sats)",
            deposit_cap
        )));
    }

    let id = format!("dep_{}", Uuid::new_v4());
    let assigned = state
        .address_pool
        .assign_address(&id)
        .await
        .map_err(server_error)?;
    let now = Utc::now();

    let deposit = Deposit {
        id: id.clone(),
        amount_sats: req.amount_sats,
        state: DepositState::Pending,
        address: assigned.address,
        target_confirmations: state.deposit_target_confirmations,
        delivery_hint: req.delivery_hint,
        metadata: req.metadata,
        txid: None,
        confirmations: 0,
        last_checked_at: None,
        created_at: now,
        updated_at: now,
        minted_token: None,
        token: None,
        minted_amount_sats: None,
        token_ready_at: None,
        mint_attempt_count: 0,
        last_mint_attempt_at: None,
        mint_error: None,
        delivery_attempt_count: 0,
        last_delivery_attempt_at: None,
        delivery_error: None,
    };

    state
        .db
        .insert_deposit(&deposit)
        .await
        .map_err(server_error)?;

    Ok(Json(ApiResponse { data: deposit }))
}

async fn get_deposit(State(state): State<AppState>, Path(id): Path<String>) -> ApiResult<Deposit> {
    match state.db.fetch_deposit(&id).await {
        Ok(dep) => Ok(Json(ApiResponse { data: dep })),
        Err(sqlx::Error::RowNotFound) => Err(not_found("deposit_not_found")),
        Err(err) => Err(server_error(err)),
    }
}

async fn request_withdrawal(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<WithdrawalRequest>,
) -> ApiResult<WithdrawalView> {
    if req.amount_sats == 0 {
        return Err(invalid_request("amount_sats must be greater than zero"));
    }
    if req.amount_sats < state.withdrawal_min_sats {
        return Err(invalid_request(
            "amount_sats is below the minimum withdrawal limit",
        ));
    }
    if req.delivery_address.trim().is_empty() {
        return Err(invalid_request("delivery_address is required"));
    }

    let token_raw = req
        .token
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());

    if token_raw.is_none() && !req.create_payment_request {
        return Err(invalid_request(
            "token is required unless create_payment_request is true",
        ));
    }
    if token_raw.is_some() && req.create_payment_request {
        return Err(invalid_request(
            "token and create_payment_request cannot both be provided",
        ));
    }

    let available_onchain = match state.onchain_wallet.as_ref() {
        Some(wallet) => {
            let summary = wallet.balance().await.map_err(server_error)?;
            summary.confirmed + summary.trusted_pending
        }
        None => return Err(unavailable("on-chain wallet not configured")),
    };
    let ratio = state.single_request_cap_ratio;
    let withdrawal_cap = ((available_onchain as f64) * ratio).floor() as u64;
    if withdrawal_cap == 0 {
        return Err(unavailable(
            "on-chain wallet is depleted; please contact the operator",
        ));
    }
    if req.amount_sats > withdrawal_cap {
        tracing::warn!(
            target: "backend",
            requested = req.amount_sats,
            available_onchain,
            withdrawal_cap,
            "withdrawal request exceeds on-chain float cap"
        );
        return Err(invalid_request(format!(
            "amount_sats exceeds the current on-chain float cap ({} sats)",
            withdrawal_cap
        )));
    }

    let id = format!("wd_{}", Uuid::new_v4());
    let now = Utc::now();

    let mut withdrawal = Withdrawal {
        id: id.clone(),
        state: WithdrawalState::Queued,
        delivery_address: req.delivery_address.trim().to_string(),
        max_fee_sats: req.max_fee_sats,
        requested_amount_sats: Some(req.amount_sats),
        token_value_sats: None,
        token: token_raw.map(ToOwned::to_owned),
        txid: None,
        error: None,
        last_attempt_at: None,
        attempt_count: 0,
        created_at: now,
        updated_at: now,
        token_consumed: false,
        swap_fee_sats: None,
        payment_request_id: None,
        payment_request_creq: None,
        payment_request_expires_at: None,
        payment_request_fulfilled_at: None,
    };

    let known_mint = if let Some(token) = &withdrawal.token {
        let mint_url = token_mint_url(token).map_err(map_token_error)?;
        let token_amount = token_total_amount(token).map_err(map_token_error)?;
        if token_amount < req.amount_sats {
            return Err(invalid_request("token value is below requested amount"));
        }
        Some(mint_url)
    } else {
        let canonical_mint = match state.cashu_mint_url.as_deref() {
            Some(mint) => mint.to_string(),
            None => return Err(unavailable("cashu mint not configured")),
        };
        let base_url = match infer_base_url(&headers) {
            Some(url) => url,
            None => return Err(invalid_request("missing Host header")),
        };
        let payment_request_id = format!("pr_{}", Uuid::new_v4());
        let transport_target = format!(
            "{}/api/v1/withdrawals/{}/nut18",
            base_url.trim_end_matches('/'),
            id
        );
        let expires_at = now + Duration::seconds(PAYMENT_REQUEST_TTL_SECS);
        let description = format!("Cashu → Bitcoin withdrawal {} sats", req.amount_sats);
        let request = Nut18PaymentRequest {
            id: Some(payment_request_id.clone()),
            amount: Some(req.amount_sats),
            unit: Some("sat".to_string()),
            single_use: Some(true),
            mints: Some(vec![canonical_mint.clone()]),
            description: Some(description),
            transports: Some(vec![Nut18Transport {
                kind: "post".to_string(),
                target: transport_target,
                tags: None,
            }]),
            nut10: None,
        };
        let encoded = encode_payment_request(&request).map_err(server_error)?;
        withdrawal.state = WithdrawalState::Funding;
        withdrawal.payment_request_id = Some(payment_request_id);
        withdrawal.payment_request_creq = Some(encoded);
        withdrawal.payment_request_expires_at = Some(expires_at);
        Some(canonical_mint)
    };

    state
        .db
        .insert_withdrawal(&withdrawal)
        .await
        .map_err(server_error)?;

    Ok(Json(ApiResponse {
        data: WithdrawalView::new(withdrawal, state.cashu_mint_url.as_deref(), known_mint),
    }))
}

async fn get_withdrawal(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<WithdrawalView> {
    match state.db.fetch_withdrawal(&id).await {
        Ok(wd) => Ok(Json(ApiResponse {
            data: WithdrawalView::new(wd, state.cashu_mint_url.as_deref(), None),
        })),
        Err(sqlx::Error::RowNotFound) => Err(not_found("withdrawal_not_found")),
        Err(err) => Err(server_error(err)),
    }
}

async fn submit_payment_request(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(payload): Json<Nut18PaymentPayload>,
) -> ApiResult<PaymentAcceptResponse> {
    let withdrawal = match state.db.fetch_withdrawal(&id).await {
        Ok(wd) => wd,
        Err(sqlx::Error::RowNotFound) => return Err(not_found("withdrawal_not_found")),
        Err(err) => return Err(server_error(err)),
    };

    let proofs_len = payload.proofs.len();
    let string_proofs = payload
        .proofs
        .iter()
        .filter(|value| value.is_string())
        .count();
    let proof_shape = payload.proofs.get(0).map(json_kind).unwrap_or("none");
    let amount_kind = payload
        .proofs
        .get(0)
        .and_then(|proof| proof.get("amount"))
        .map(json_kind)
        .unwrap_or("missing");
    tracing::info!(
        target: "backend",
        withdrawal_id = %withdrawal.id,
        payment_id = ?payload.id,
        mint = %payload.mint,
        unit = %payload.unit,
        proofs = proofs_len,
        string_proofs,
        proof_shape,
        amount_kind,
        state = %withdrawal.state.as_str(),
        "received nut18 payment payload"
    );

    if withdrawal.state != WithdrawalState::Funding {
        tracing::warn!(
            target: "backend",
            withdrawal_id = %withdrawal.id,
            state = %withdrawal.state.as_str(),
            "payment request is not accepting funding"
        );
        return Err(conflict("withdrawal_not_accepting_payment"));
    }
    if withdrawal.payment_request_fulfilled_at.is_some() {
        tracing::warn!(
            target: "backend",
            withdrawal_id = %withdrawal.id,
            "payment request already fulfilled"
        );
        return Err(conflict("payment_request_already_fulfilled"));
    }
    if withdrawal.payment_request_id.is_none() || withdrawal.payment_request_creq.is_none() {
        tracing::error!(
            target: "backend",
            withdrawal_id = %withdrawal.id,
            "payment request metadata missing on withdrawal"
        );
        return Err(unavailable("payment_request_not_configured"));
    }
    if let Some(expected) = withdrawal.payment_request_id.as_deref() {
        if payload.id.as_deref() != Some(expected) {
            tracing::warn!(
                target: "backend",
                withdrawal_id = %withdrawal.id,
                expected_id = %expected,
                actual_id = ?payload.id,
                "payment request id mismatch"
            );
            return Err(invalid_request("payment_request_id_mismatch"));
        }
    }
    if let Some(expires_at) = withdrawal.payment_request_expires_at {
        if Utc::now() > expires_at {
            tracing::warn!(
                target: "backend",
                withdrawal_id = %withdrawal.id,
                expires_at = %expires_at,
                "payment request expired"
            );
            return Err(invalid_request("payment_request_expired"));
        }
    }
    if payload.proofs.is_empty() {
        tracing::warn!(
            target: "backend",
            withdrawal_id = %withdrawal.id,
            "payment request payload missing proofs"
        );
        return Err(invalid_request("proofs must not be empty"));
    }
    if payload.unit.to_ascii_lowercase() != "sat" {
        tracing::warn!(
            target: "backend",
            withdrawal_id = %withdrawal.id,
            unit = %payload.unit,
            "payment request payload has invalid unit"
        );
        return Err(invalid_request("unit must be 'sat'"));
    }
    if let Some(expected) = state.cashu_mint_url.as_deref() {
        let expected_norm = normalize_mint_url(expected);
        let actual_norm = normalize_mint_url(&payload.mint);
        if actual_norm != expected_norm {
            tracing::warn!(
                target: "backend",
                withdrawal_id = %withdrawal.id,
                expected_mint = %expected,
                actual_mint = %payload.mint,
                "payment request mint mismatch"
            );
            return Err(invalid_request("mint_mismatch"));
        }
    }

    let withdrawal_id = withdrawal.id.clone();
    let normalized_proofs: Vec<serde_json::Value> = payload
        .proofs
        .iter()
        .map(|value| {
            if let serde_json::Value::String(inner) = value {
                match serde_json::from_str(inner) {
                    Ok(parsed) => parsed,
                    Err(err) => {
                        tracing::warn!(
                            target: "backend",
                            withdrawal_id = %withdrawal_id,
                            error = %err,
                            "failed to parse proof string payload"
                        );
                        value.clone()
                    }
                }
            } else if let serde_json::Value::Object(mut map) = value.clone() {
                if let Some(amount_value) = map.get_mut("amount") {
                    if let serde_json::Value::String(amount_str) = amount_value {
                        match amount_str.parse::<u64>() {
                            Ok(parsed) => {
                                *amount_value = serde_json::Value::Number(parsed.into());
                            }
                            Err(err) => {
                                tracing::warn!(
                                    target: "backend",
                                    withdrawal_id = %withdrawal_id,
                                    error = %err,
                                    "failed to parse proof amount string"
                                );
                            }
                        }
                    }
                }
                serde_json::Value::Object(map)
            } else {
                value.clone()
            }
        })
        .collect();

    let token_json = json!({
        "token": [{
            "mint": payload.mint,
            "unit": payload.unit,
            "proofs": normalized_proofs,
        }]
    });
    let token_body = serde_json::to_string(&token_json).map_err(server_error)?;
    if let Err(err) = fs::write("/tmp/nut18-last-token.json", &token_body) {
        tracing::warn!(
            target: "backend",
            withdrawal_id = %withdrawal.id,
            error = %err,
            "failed to write debug token snapshot"
        );
    }
    let encoded = format!("cashuA{}", URL_SAFE_NO_PAD.encode(token_body.as_bytes()));
    let amount = match token_total_amount(&encoded) {
        Ok(value) => value,
        Err(err) => {
            tracing::warn!(
                target: "backend",
                withdrawal_id = %withdrawal.id,
                error = %err,
                "failed to decode payment request token"
            );
            return Err(map_token_error(err));
        }
    };
    if let Some(expected) = withdrawal.requested_amount_sats {
        if amount < expected {
            tracing::warn!(
                target: "backend",
                withdrawal_id = %withdrawal.id,
                amount_sats = amount,
                expected_sats = expected,
                "payment amount below requested amount"
            );
            return Err(invalid_request("payment amount below requested amount"));
        }
    }

    state
        .db
        .record_payment_request_token(&withdrawal.id, &encoded)
        .await
        .map_err(|err| match err {
            sqlx::Error::RowNotFound => {
                tracing::warn!(
                    target: "backend",
                    withdrawal_id = %withdrawal.id,
                    "payment request row missing when recording token"
                );
                conflict("withdrawal_not_accepting_payment")
            }
            other => {
                tracing::error!(
                    target: "backend",
                    withdrawal_id = %withdrawal.id,
                    error = %other,
                    "failed to record payment request token"
                );
                server_error(other)
            }
        })?;

    tracing::info!(
        target: "backend",
        withdrawal_id = %withdrawal.id,
        payment_id = ?payload.id,
        amount_sats = amount,
        proofs = proofs_len,
        "payment request accepted"
    );

    Ok(Json(ApiResponse {
        data: PaymentAcceptResponse {
            accepted: true,
            amount_sats: amount,
        },
    }))
}

async fn get_wallet_balance(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> ApiResult<WalletBalanceResponse> {
    let wallet = wallet_guard(&state, &headers)?;
    let summary = wallet.balance().await.map_err(server_error)?;
    Ok(Json(ApiResponse {
        data: WalletBalanceResponse::from(summary),
    }))
}

async fn send_wallet_payment(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<WalletSendRequest>,
) -> ApiResult<WalletSendResponse> {
    if req.amount_sats == 0 {
        return Err(invalid_request("amount_sats must be greater than zero"));
    }
    if req.fee_rate_vb <= 0.0 {
        return Err(invalid_request("fee_rate_vb must be greater than zero"));
    }
    if req.address.trim().is_empty() {
        return Err(invalid_request("address is required"));
    }

    let wallet = wallet_guard(&state, &headers)?;
    let txid = wallet
        .send_to_address(&req.address, req.amount_sats, req.fee_rate_vb)
        .await
        .map_err(server_error)?;

    Ok(Json(ApiResponse {
        data: WalletSendResponse { txid },
    }))
}

async fn get_wallet_topup(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> ApiResult<WalletTopUpResponse> {
    let wallet = wallet_guard(&state, &headers)?;
    let address = wallet.next_external_address().await.map_err(server_error)?;
    let bip21_uri = format_bip21(&address, Some(WALLET_TOPUP_LABEL));
    Ok(Json(ApiResponse {
        data: WalletTopUpResponse { address, bip21_uri },
    }))
}

async fn sync_wallet(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> ApiResult<WalletBalanceResponse> {
    let wallet = wallet_guard(&state, &headers)?;
    state.metrics.inc_wallet_sync();
    let started = Instant::now();
    if let Err(err) = wallet.sync().await {
        state.metrics.inc_wallet_sync_failure();
        return Err(server_error(err));
    }
    let summary = match wallet.balance().await {
        Ok(summary) => summary,
        Err(err) => {
            state.metrics.inc_wallet_sync_failure();
            return Err(server_error(err));
        }
    };
    let elapsed_ms = started.elapsed().as_millis() as u64;
    tracing::info!(
        target: "backend",
        elapsed_ms,
        confirmed = summary.confirmed,
        trusted_pending = summary.trusted_pending,
        untrusted_pending = summary.untrusted_pending,
        immature = summary.immature,
        "manual wallet sync completed"
    );

    Ok(Json(ApiResponse {
        data: WalletBalanceResponse::from(summary),
    }))
}

async fn create_cashu_invoice(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<CashuInvoiceRequest>,
) -> ApiResult<CashuInvoiceStatusResponse> {
    if req.amount_sats == 0 {
        return Err(invalid_request("amount_sats must be greater than zero"));
    }

    let wallet = cashu_guard(&state, &headers)?;
    let method = if req.bolt12 {
        KnownMethod::Bolt12
    } else {
        KnownMethod::Bolt11
    };
    let quote = {
        let guard = wallet.lock().await;
        guard
            .mint_quote(method, Some(Amount::from(req.amount_sats)), None, None)
            .await
            .map_err(server_error)?
    };

    Ok(Json(ApiResponse {
        data: map_quote_response(quote),
    }))
}

async fn get_cashu_invoice(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> ApiResult<CashuInvoiceStatusResponse> {
    let wallet = cashu_guard(&state, &headers)?;
    let quote = {
        let guard = wallet.lock().await;
        guard
            .check_mint_quote_status(&id)
            .await
            .map_err(server_error)?
    };
    Ok(Json(ApiResponse {
        data: map_quote_response(quote),
    }))
}

async fn mint_cashu_invoice(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> ApiResult<CashuMintResponse> {
    let wallet = cashu_guard(&state, &headers)?;
    let proofs = {
        let guard = wallet.lock().await;
        guard
            .mint(&id, SplitTarget::default(), None)
            .await
            .map_err(server_error)?
    };
    let amount_sats = proofs
        .iter()
        .map(|proof| proof.amount.clone().to_u64())
        .sum();

    Ok(Json(ApiResponse {
        data: CashuMintResponse {
            minted: true,
            amount_sats,
        },
    }))
}

async fn get_cashu_wallet_balance(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> ApiResult<CashuWalletBalanceResponse> {
    let wallet = cashu_guard(&state, &headers)?;
    let (spendable, pending, reserved) = {
        let guard = wallet.lock().await;
        let spendable = guard.total_balance().await.map_err(server_error)?.to_u64();
        let pending = guard
            .total_pending_balance()
            .await
            .map_err(server_error)?
            .to_u64();
        let reserved = guard
            .total_reserved_balance()
            .await
            .map_err(server_error)?
            .to_u64();
        (spendable, pending, reserved)
    };

    Ok(Json(ApiResponse {
        data: CashuWalletBalanceResponse {
            spendable,
            pending,
            reserved,
        },
    }))
}

async fn send_cashu_token(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<CashuSendRequest>,
) -> ApiResult<CashuSendResponse> {
    if req.amount_sats == 0 {
        return Err(invalid_request("amount_sats must be greater than zero"));
    }
    let wallet = cashu_guard(&state, &headers)?;
    let token = {
        let guard = wallet.lock().await;
        let prepared = guard
            .prepare_send(Amount::from(req.amount_sats), SendOptions::default())
            .await
            .map_err(server_error)?;
        let confirmed = prepared.confirm(None).await.map_err(server_error)?;
        confirmed.to_string()
    };

    Ok(Json(ApiResponse {
        data: CashuSendResponse { token },
    }))
}

async fn inspect_cashu_token(
    State(state): State<AppState>,
    Json(req): Json<TokenInspectRequest>,
) -> ApiResult<TokenInspectResponse> {
    let token_raw = req.token.trim();
    if token_raw.is_empty() {
        return Err(invalid_request("token is required"));
    }
    let mint_url = token_mint_url(token_raw).map_err(map_token_error)?;
    let is_foreign = state
        .cashu_mint_url
        .as_deref()
        .map(|expected| expected != mint_url)
        .unwrap_or(false);

    Ok(Json(ApiResponse {
        data: TokenInspectResponse {
            mint_url,
            is_foreign,
        },
    }))
}

async fn get_float_status(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> ApiResult<FloatStatusResponse> {
    require_operator_token(&state, &headers)?;
    let snapshot = state.float_status.read().await.clone();
    let total_balance = snapshot.onchain.balance_sats + snapshot.cashu.balance_sats;
    let drift = state.float_target_sats as i64 - total_balance as i64;

    Ok(Json(ApiResponse {
        data: FloatStatusResponse {
            target_sats: state.float_target_sats,
            min_ratio: state.float_min_ratio,
            max_ratio: state.float_max_ratio,
            onchain: WalletFloatStatusPayload::from(&snapshot.onchain),
            cashu: WalletFloatStatusPayload::from(&snapshot.cashu),
            total_balance_sats: total_balance,
            drift_sats: drift,
        },
    }))
}

fn encode_payment_request(request: &Nut18PaymentRequest) -> Result<String, serde_cbor::Error> {
    let cbor = serde_cbor::to_vec(request)?;
    Ok(format!("creqA{}", URL_SAFE_NO_PAD.encode(cbor)))
}

fn normalize_mint_url(value: &str) -> &str {
    value.trim_end_matches('/')
}

fn json_kind(value: &serde_json::Value) -> &'static str {
    match value {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "bool",
        serde_json::Value::Number(_) => "number",
        serde_json::Value::String(_) => "string",
        serde_json::Value::Array(_) => "array",
        serde_json::Value::Object(_) => "object",
    }
}

fn infer_base_url(headers: &HeaderMap) -> Option<String> {
    let host = headers
        .get("x-forwarded-host")
        .or_else(|| headers.get(HOST))?
        .to_str()
        .ok()?;
    let proto = headers
        .get("x-forwarded-proto")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("http");
    Some(format!("{}://{}", proto, host))
}

fn wallet_guard(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<Arc<OnchainWallet>, (StatusCode, Json<ApiError>)> {
    require_operator_token(state, headers)?;
    state
        .onchain_wallet
        .clone()
        .ok_or_else(|| unavailable("on-chain wallet not configured"))
}

fn cashu_guard(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<WalletHandle, (StatusCode, Json<ApiError>)> {
    require_operator_token(state, headers)?;
    state
        .cashu_wallet
        .clone()
        .ok_or_else(|| unavailable("cashu wallet not configured"))
}

fn require_operator_token(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<(), (StatusCode, Json<ApiError>)> {
    let token = state
        .wallet_api_token
        .as_deref()
        .ok_or_else(|| unavailable("wallet API token not configured"))?;
    let provided = headers
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|raw| raw.strip_prefix("Bearer "))
        .unwrap_or("");
    if provided != token {
        return Err(unauthorized());
    }
    Ok(())
}

fn format_bip21(address: &str, label: Option<&str>) -> String {
    match label {
        Some(label) if !label.is_empty() => {
            format!("bitcoin:{}?label={}", address, encode(label))
        }
        _ => format!("bitcoin:{}", address),
    }
}

fn payment_method_label(method: &PaymentMethod) -> &'static str {
    match method {
        PaymentMethod::Known(KnownMethod::Bolt11) => "bolt11",
        PaymentMethod::Known(KnownMethod::Bolt12) => "bolt12",
        _ => "custom",
    }
}

fn mint_state_label(state: MintQuoteState) -> &'static str {
    match state {
        MintQuoteState::Unpaid => "unpaid",
        MintQuoteState::Paid => "paid",
        MintQuoteState::Issued => "issued",
    }
}

fn map_quote_response(quote: MintQuote) -> CashuInvoiceStatusResponse {
    let MintQuote {
        id,
        amount,
        amount_paid,
        amount_issued,
        request,
        payment_method,
        state,
        expiry,
        ..
    } = quote;

    CashuInvoiceStatusResponse {
        quote_id: id,
        amount_sats: amount.map(|a| a.to_u64()).unwrap_or(0),
        amount_paid_sats: amount_paid.to_u64(),
        amount_issued_sats: amount_issued.to_u64(),
        request,
        method: payment_method_label(&payment_method).to_string(),
        state: mint_state_label(state).to_string(),
        expires_at: expiry,
    }
}

fn map_token_error(err: TokenMintError) -> (StatusCode, Json<ApiError>) {
    match err {
        TokenMintError::Malformed => invalid_request("token is malformed"),
        TokenMintError::MultiMint => {
            invalid_request("token references multiple or unsupported mints")
        }
    }
}

fn invalid_request(message: impl Into<String>) -> (StatusCode, Json<ApiError>) {
    (
        StatusCode::BAD_REQUEST,
        Json(ApiError {
            code: "validation_error",
            message: message.into(),
        }),
    )
}

fn conflict(code: &'static str) -> (StatusCode, Json<ApiError>) {
    (
        StatusCode::CONFLICT,
        Json(ApiError {
            code,
            message: "Resource state conflict".into(),
        }),
    )
}

fn unavailable(message: &str) -> (StatusCode, Json<ApiError>) {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(ApiError {
            code: "wallet_unavailable",
            message: message.to_string(),
        }),
    )
}

fn unauthorized() -> (StatusCode, Json<ApiError>) {
    (
        StatusCode::UNAUTHORIZED,
        Json(ApiError {
            code: "unauthorized",
            message: "invalid or missing wallet API token".into(),
        }),
    )
}

fn not_found(code: &'static str) -> (StatusCode, Json<ApiError>) {
    (
        StatusCode::NOT_FOUND,
        Json(ApiError {
            code,
            message: "Resource not found".into(),
        }),
    )
}

fn server_error<E: std::fmt::Display>(err: E) -> (StatusCode, Json<ApiError>) {
    tracing::error!(target: "backend", error = %err, "internal error");
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(ApiError {
            code: "server_error",
            message: err.to_string(),
        }),
    )
}
