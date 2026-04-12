use axum::{
    Json, Router,
    body::Bytes,
    extract::{Path, Query, State},
    http::{
        HeaderMap, HeaderValue, StatusCode,
        header::{AUTHORIZATION, CONTENT_TYPE},
    },
    response::IntoResponse,
    routing::{get, post},
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::{Duration, Utc};
use rand::{RngCore, rngs::OsRng};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    borrow::ToOwned,
    collections::HashMap,
    fs,
    sync::{Arc, Mutex, OnceLock},
    time::Instant,
};
use uuid::Uuid;

use crate::AppState;
use backend::cashu::{TokenMintError, token_fingerprint, token_mint_url, token_total_amount};
use backend::db::{Deposit, DepositState, Session, StateLiabilityRow, Withdrawal, WithdrawalState};
use backend::fees::FeeEstimateSnapshot;
use backend::onchain::{OnchainBalance, OnchainWallet};
use backend::operations::OperationMode;
use backend::wallet::WalletHandle;
use cdk::Amount;
use cdk::amount::SplitTarget;
use cdk::nuts::Token;
use cdk::nuts::nut00::KnownMethod;
use cdk::nuts::nut00::token::TokenV3;
use cdk::nuts::{MintQuoteState, PaymentMethod};
use cdk::wallet::{MintQuote, SendOptions};
use urlencoding::encode;

type ApiResult<T> = Result<Json<ApiResponse<T>>, (StatusCode, Json<ApiError>)>;
const WALLET_TOPUP_LABEL: &str = "Shuestand hot wallet top-up";
const PAYMENT_REQUEST_TTL_SECS: i64 = 300;
const SESSION_HEADER: &str = "x-shuestand-session";
const SESSION_TOKEN_PREFIX: &str = "st_";
const SESSION_TOKEN_BYTES: usize = 16;
const SESSION_TOKEN_HEX_LEN: usize = SESSION_TOKEN_BYTES * 2;
const SESSION_CLAIM_GROUP_LEN: usize = 8;

const DEFAULT_OPERATOR_WITHDRAWAL_LIMIT: usize = 50;
const DEFAULT_WITHDRAWAL_STATES: [WithdrawalState; 5] = [
    WithdrawalState::Funding,
    WithdrawalState::Queued,
    WithdrawalState::Broadcasting,
    WithdrawalState::Confirming,
    WithdrawalState::Failed,
];
const DEFAULT_DEPOSIT_STATES: [DepositState; 6] = [
    DepositState::Pending,
    DepositState::Confirming,
    DepositState::Minting,
    DepositState::Delivering,
    DepositState::Ready,
    DepositState::Failed,
];
const CASHU_WALLET_UNAVAILABLE_MESSAGE: &str =
    "Cashu wallet unavailable; please contact the operator";
const CASHU_FLOAT_DEPLETED_MESSAGE: &str = "Cashu float is depleted; please contact the operator";
const DEPOSIT_FLOAT_TOO_LOW_MESSAGE: &str = "Float too low, please contact operator";
const SUSPICIOUS_TOKEN_RATIO_THRESHOLD: f64 = 1.5;
const WITHDRAWAL_BURST_WINDOW_SECS: i64 = 60;
const WITHDRAWAL_BURST_THRESHOLD: usize = 4;
const OPERATOR_401_WINDOW_SECS: i64 = 60;
const OPERATOR_401_THRESHOLD: usize = 20;

static SECURITY_WINDOW_BUCKETS: OnceLock<Mutex<HashMap<String, Vec<i64>>>> = OnceLock::new();

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(healthcheck))
        .route("/metrics", get(export_metrics))
        .route("/api/v1/config", get(get_public_config))
        .route("/api/v1/sessions", post(start_session))
        .route("/api/v1/sessions/resume", post(resume_session))
        .route("/api/v1/deposits", post(create_deposit))
        .route("/api/v1/deposits/:id", get(get_deposit))
        .route("/api/v1/deposits/:id/pickup", post(pickup_deposit))
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
        .route("/api/v1/operator/ledger", get(get_ledger_snapshot))
        .route("/api/v1/operator/deposits", get(list_operator_deposits))
        .route(
            "/api/v1/operator/deposits/:id/actions",
            post(operate_deposit),
        )
        .route(
            "/api/v1/operator/withdrawals",
            get(list_operator_withdrawals),
        )
        .route(
            "/api/v1/operator/withdrawals/:id/actions",
            post(operate_withdrawal),
        )
        .route(
            "/api/v1/operator/transactions/counter",
            get(get_transaction_counter),
        )
        .route(
            "/api/v1/operator/mode",
            get(get_operation_mode).post(update_operation_mode),
        )
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

#[derive(Serialize)]
struct PublicConfigResponse {
    withdrawal_min_sats: u64,
    deposit_min_sats: u64,
    deposit_target_confirmations: u8,
    float_target_sats: u64,
    float_min_ratio: f32,
    float_max_ratio: f32,
    single_request_cap_ratio: f64,
    deposit_flow_enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    deposit_flow_reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    cashu_mint_url: Option<String>,
    fee_estimates: FeeEstimatesPayload,
}

async fn get_public_config(State(state): State<AppState>) -> ApiResult<PublicConfigResponse> {
    let (deposit_flow_enabled, deposit_flow_reason) = if state.cashu_wallet.is_none() {
        (false, Some(CASHU_WALLET_UNAVAILABLE_MESSAGE.to_string()))
    } else {
        let snapshot = state.float_status.read().await.clone();
        let cashu_balance = snapshot.cashu.balance_sats;
        if cashu_balance == 0 {
            (false, Some(CASHU_FLOAT_DEPLETED_MESSAGE.to_string()))
        } else {
            let deposit_cap =
                ((cashu_balance as f64) * state.single_request_cap_ratio).floor() as u64;
            if deposit_cap < state.withdrawal_min_sats {
                (false, Some(DEPOSIT_FLOAT_TOO_LOW_MESSAGE.to_string()))
            } else {
                (true, None)
            }
        }
    };

    let fee_snapshot = state.fee_estimator.snapshot().await;

    let payload = PublicConfigResponse {
        withdrawal_min_sats: state.withdrawal_min_sats,
        deposit_min_sats: state.deposit_min_sats,
        deposit_target_confirmations: state.deposit_target_confirmations,
        float_target_sats: state.float_target_sats,
        float_min_ratio: state.float_min_ratio,
        float_max_ratio: state.float_max_ratio,
        single_request_cap_ratio: state.single_request_cap_ratio,
        deposit_flow_enabled,
        deposit_flow_reason,
        cashu_mint_url: state.cashu_mint_url.clone(),
        fee_estimates: FeeEstimatesPayload::from_snapshot(fee_snapshot),
    };

    Ok(Json(ApiResponse { data: payload }))
}

#[derive(Deserialize, Debug)]
struct DepositRequest {
    amount_sats: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    delivery_hint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    metadata: Option<serde_json::Value>,
}

#[derive(Serialize)]
struct DepositCreateResponse {
    deposit: Deposit,
    pickup_token: String,
}

#[derive(Serialize)]
struct SessionStartResponse {
    session_id: String,
    token: String,
    claim_code: String,
    expires_at: String,
}

#[derive(Deserialize)]
struct SessionResumeRequest {
    claim_code: String,
}

type SessionResumeResponse = SessionStartResponse;

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

#[derive(Deserialize)]
struct DepositPickupRequest {
    pickup_token: String,
}

#[derive(Serialize)]
struct DepositPickupResponse {
    token: String,
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

#[derive(Serialize)]
struct OperationModeResponse {
    mode: OperationMode,
}

#[derive(Deserialize)]
struct OperationModeUpdateRequest {
    mode: OperationMode,
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

#[derive(Deserialize, Default)]
struct OperatorListQuery {
    #[serde(default)]
    state: Vec<String>,
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(Deserialize)]
struct WithdrawalActionRequest {
    action: WithdrawalActionKind,
    #[serde(default)]
    note: Option<String>,
    #[serde(default)]
    txid: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum WithdrawalActionKind {
    MarkSettled,
    MarkFailed,
    Requeue,
    Archive,
}

#[derive(Deserialize)]
struct DepositActionRequest {
    action: DepositActionKind,
    #[serde(default)]
    note: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum DepositActionKind {
    MarkFulfilled,
    MarkFailed,
    Archive,
}

#[derive(Serialize)]
struct LedgerSnapshotResponse {
    captured_at: String,
    float_target_sats: u64,
    cashu: CashuLedgerResponse,
    onchain: OnchainLedgerResponse,
    totals: LedgerTotalsResponse,
}

#[derive(Serialize)]
struct FeeEstimatesPayload {
    fast: FeeEstimateEntryPayload,
    economy: FeeEstimateEntryPayload,
}

#[derive(Serialize)]
struct FeeEstimateEntryPayload {
    sats_per_vb: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    updated_at: Option<String>,
}

impl FeeEstimatesPayload {
    fn from_snapshot(snapshot: FeeEstimateSnapshot) -> Self {
        Self {
            fast: FeeEstimateEntryPayload::from_entry(snapshot.fast),
            economy: FeeEstimateEntryPayload::from_entry(snapshot.economy),
        }
    }
}

impl FeeEstimateEntryPayload {
    fn from_entry(entry: backend::fees::FeeEstimateEntry) -> Self {
        Self {
            sats_per_vb: entry.sats_per_vb,
            updated_at: entry.updated_at.map(|ts| ts.to_rfc3339()),
        }
    }
}

#[derive(Serialize)]
struct TransactionCounterResponse {
    count: i64,
}

#[derive(Serialize)]
struct LedgerTotalsResponse {
    assets_sats: u64,
    liabilities_sats: u64,
    net_sats: i64,
}

#[derive(Serialize)]
struct CashuLedgerResponse {
    assets: CashuAssetBreakdown,
    liabilities: CashuLiabilityBreakdown,
    net_sats: i64,
}

#[derive(Serialize)]
struct CashuAssetBreakdown {
    spendable: u64,
    pending: u64,
    reserved: u64,
    available_sats: u64,
}

#[derive(Serialize)]
struct CashuLiabilityBreakdown {
    awaiting_confirmations: LiabilityBucket,
    minting: LiabilityBucket,
    ready: LiabilityBucket,
    total_sats: u64,
}

#[derive(Serialize)]
struct LiabilityBucket {
    amount_sats: u64,
    count: u64,
}

#[derive(Serialize)]
struct OnchainLedgerResponse {
    assets: OnchainAssetBreakdown,
    liabilities: OnchainLiabilityBreakdown,
    net_sats: i64,
}

#[derive(Serialize)]
struct OnchainAssetBreakdown {
    confirmed: u64,
    trusted_pending: u64,
    untrusted_pending: u64,
    immature: u64,
    available_sats: u64,
}

#[derive(Serialize)]
struct OnchainLiabilityBreakdown {
    funding: LiabilityBucket,
    queued: LiabilityBucket,
    broadcasting: LiabilityBucket,
    confirming: LiabilityBucket,
    total_sats: u64,
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

async fn start_session(State(state): State<AppState>) -> ApiResult<SessionStartResponse> {
    let now = Utc::now();
    let expires_at = now + state.session_ttl;
    let (token, claim_code) = generate_session_token();
    let session = Session {
        id: format!("sess_{}", Uuid::new_v4()),
        token_hash: hash_session_token(&token),
        created_at: now,
        last_seen_at: now,
        expires_at,
    };

    if let Err(err) = state.db.delete_expired_sessions(now).await {
        tracing::warn!(target: "backend", error = %err, "failed to purge expired sessions");
    }

    state
        .db
        .create_session(&session)
        .await
        .map_err(server_error)?;

    Ok(Json(ApiResponse {
        data: SessionStartResponse {
            session_id: session.id,
            token,
            claim_code,
            expires_at: expires_at.to_rfc3339(),
        },
    }))
}

async fn resume_session(
    State(state): State<AppState>,
    Json(req): Json<SessionResumeRequest>,
) -> ApiResult<SessionResumeResponse> {
    let now = Utc::now();
    if let Err(err) = state.db.delete_expired_sessions(now).await {
        tracing::warn!(target: "backend", error = %err, "failed to purge expired sessions");
    }

    let normalized = match normalize_claim_code(&req.claim_code) {
        Some(payload) => payload,
        None => return Err(invalid_request("invalid_claim_code")),
    };
    let token = format!("{}{}", SESSION_TOKEN_PREFIX, normalized);
    let token_hash = hash_session_token(&token);

    let mut session = match state.db.fetch_session_by_token_hash(&token_hash).await {
        Ok(sess) => sess,
        Err(sqlx::Error::RowNotFound) => return Err(invalid_request("invalid_claim_code")),
        Err(err) => return Err(server_error(err)),
    };

    if session.expires_at < now {
        return Err(invalid_request("session_expired"));
    }

    let new_expiry = now + state.session_ttl;
    state
        .db
        .touch_session(&session.id, now, new_expiry)
        .await
        .map_err(server_error)?;
    session.last_seen_at = now;
    session.expires_at = new_expiry;

    Ok(Json(ApiResponse {
        data: SessionStartResponse {
            session_id: session.id,
            token,
            claim_code: claim_code_from_token_payload(&normalized),
            expires_at: new_expiry.to_rfc3339(),
        },
    }))
}

async fn create_deposit(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<DepositRequest>,
) -> ApiResult<DepositCreateResponse> {
    const MAX_DEPOSIT_SATS: u64 = super::MAX_DEPOSIT_SATS;
    let min_deposit_sats = state.deposit_min_sats;

    match state.current_operation_mode().await {
        OperationMode::Normal => {}
        mode => return Err(mode_blocked(mode, "deposits")),
    }

    if req.amount_sats < min_deposit_sats || req.amount_sats > MAX_DEPOSIT_SATS {
        return Err(invalid_request(format!(
            "amount_sats must be between {} and {} sats",
            min_deposit_sats, MAX_DEPOSIT_SATS
        )));
    }

    let cashu_wallet = state
        .cashu_wallet
        .as_ref()
        .ok_or_else(|| unavailable("cashu wallet not configured"))?;
    let spendable = {
        let guard = cashu_wallet.lock().await;
        guard.total_balance().await.map_err(server_error)?.to_u64()
    };
    let reserved_cashu = state
        .db
        .reserved_cashu_deposit_sats()
        .await
        .map_err(server_error)?;
    let effective_spendable = spendable.saturating_sub(reserved_cashu);
    let ratio = state.single_request_cap_ratio;
    let deposit_cap = ((effective_spendable as f64) * ratio).floor() as u64;
    if deposit_cap == 0 {
        return Err(unavailable(
            "cashu float is depleted; please contact the operator",
        ));
    }
    if deposit_cap < state.withdrawal_min_sats {
        tracing::warn!(
            target: "backend",
            spendable,
            reserved_cashu,
            effective_spendable,
            deposit_cap,
            withdrawal_min = state.withdrawal_min_sats,
            "deposit flow disabled because cap is below the withdrawal minimum"
        );
        return Err(unavailable(DEPOSIT_FLOAT_TOO_LOW_MESSAGE));
    }
    if req.amount_sats > deposit_cap {
        tracing::warn!(
            target: "backend",
            requested = req.amount_sats,
            spendable,
            reserved_cashu,
            effective_spendable,
            deposit_cap,
            "deposit request exceeds cashu float cap"
        );
        return Err(invalid_request(format!(
            "amount_sats exceeds the current transaction float cap ({} sats)",
            deposit_cap
        )));
    }

    let session = resolve_session_from_headers(&state, &headers).await?;
    let client_hint = request_client_hint(&headers);
    let session_hint = session
        .as_ref()
        .map(|s| s.id.clone())
        .unwrap_or_else(|| "none".to_string());
    if let Some(count) = detect_burst(
        "deposit",
        &format!("{}:{}", client_hint, session_hint),
        WITHDRAWAL_BURST_WINDOW_SECS,
        WITHDRAWAL_BURST_THRESHOLD,
    ) {
        emit_security_alert(
            &state,
            "deposit_burst_detected",
            json!({
                "count": count,
                "window_seconds": WITHDRAWAL_BURST_WINDOW_SECS,
                "threshold": WITHDRAWAL_BURST_THRESHOLD,
                "client": client_hint,
                "session_id": session.as_ref().map(|s| s.id.clone()),
                "delivery_hint": req.delivery_hint,
                "requested_amount_sats": req.amount_sats,
            }),
        );
    }

    let id = format!("dep_{}", Uuid::new_v4());
    let assigned = state
        .address_pool
        .assign_address(&id)
        .await
        .map_err(server_error)?;
    let now = Utc::now();

    let pickup_token = format!("pc_{}", Uuid::new_v4());

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
        session_id: session.as_ref().map(|sess| sess.id.clone()),
        pickup_token: pickup_token.clone(),
        pickup_revealed_at: None,
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

    Ok(Json(ApiResponse {
        data: DepositCreateResponse {
            deposit,
            pickup_token,
        },
    }))
}

async fn get_deposit(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> ApiResult<Deposit> {
    let session = resolve_session_from_headers(&state, &headers).await?;
    let deposit = match state.db.fetch_deposit(&id).await {
        Ok(dep) => dep,
        Err(sqlx::Error::RowNotFound) => return Err(not_found("deposit_not_found")),
        Err(err) => return Err(server_error(err)),
    };

    ensure_session_access(deposit.session_id.as_deref(), session.as_ref())?;

    Ok(Json(ApiResponse { data: deposit }))
}

async fn pickup_deposit(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(req): Json<DepositPickupRequest>,
) -> ApiResult<DepositPickupResponse> {
    let pickup_token = req.pickup_token.trim();
    if pickup_token.is_empty() {
        return Err(invalid_request("pickup_token_required"));
    }

    // Claim is one-shot. If it was already claimed (or token isn't ready), we never re-serve it.
    let minted = match state.db.claim_deposit_pickup(&id, pickup_token).await {
        Ok(token) => token,
        Err(sqlx::Error::RowNotFound) => {
            // Intentionally ambiguous to avoid leaking whether the deposit exists.
            return Err(conflict("deposit_not_ready_for_pickup"));
        }
        Err(err) => return Err(server_error(err)),
    };

    tracing::info!(
        target: "backend",
        deposit_id = %id,
        token_chars = minted.len(),
        token_fingerprint = %token_fingerprint(&minted),
        "deposit pickup served token"
    );

    if let Some(notifier) = &state.transaction_notifier {
        notifier.record_deposit(&id).await;
    }

    Ok(Json(ApiResponse {
        data: DepositPickupResponse { token: minted },
    }))
}

async fn request_withdrawal(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<WithdrawalRequest>,
) -> ApiResult<WithdrawalView> {
    match state.current_operation_mode().await {
        OperationMode::Normal => {}
        mode => return Err(mode_blocked(mode, "withdrawals")),
    }

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

    let session = resolve_session_from_headers(&state, &headers).await?;
    let client_hint = request_client_hint(&headers);
    let session_hint = session
        .as_ref()
        .map(|s| s.id.clone())
        .unwrap_or_else(|| "none".to_string());
    if let Some(count) = detect_burst(
        "withdrawal",
        &format!("{}:{}", client_hint, session_hint),
        WITHDRAWAL_BURST_WINDOW_SECS,
        WITHDRAWAL_BURST_THRESHOLD,
    ) {
        emit_security_alert(
            &state,
            "withdrawal_burst_detected",
            json!({
                "count": count,
                "window_seconds": WITHDRAWAL_BURST_WINDOW_SECS,
                "threshold": WITHDRAWAL_BURST_THRESHOLD,
                "client": client_hint,
                "session_id": session.as_ref().map(|s| s.id.clone()),
                "delivery_address": req.delivery_address,
                "requested_amount_sats": req.amount_sats,
            }),
        );
    }

    let raw_onchain = match state.onchain_wallet.as_ref() {
        Some(wallet) => {
            let summary = wallet.balance().await.map_err(server_error)?;
            summary.confirmed + summary.trusted_pending
        }
        None => return Err(unavailable("on-chain wallet not configured")),
    };
    let reserved_onchain = state
        .db
        .reserved_onchain_withdrawal_sats()
        .await
        .map_err(server_error)?;
    let available_onchain = raw_onchain.saturating_sub(reserved_onchain);
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
            raw_onchain,
            reserved_onchain,
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
        session_id: session.as_ref().map(|sess| sess.id.clone()),
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
        let ratio = (token_amount as f64) / (req.amount_sats as f64);
        if ratio >= SUSPICIOUS_TOKEN_RATIO_THRESHOLD {
            emit_security_alert(
                &state,
                "withdrawal_token_ratio_anomaly",
                json!({
                    "requested_amount_sats": req.amount_sats,
                    "token_amount_sats": token_amount,
                    "ratio": ratio,
                    "delivery_address": withdrawal.delivery_address,
                    "mint": mint_url,
                    "session_id": withdrawal.session_id,
                    "client": request_client_hint(&headers),
                }),
            );
        }
        Some(mint_url)
    } else {
        let canonical_mint = match state.cashu_mint_url.as_deref() {
            Some(mint) => mint.to_string(),
            None => return Err(unavailable("cashu mint not configured")),
        };
        let base_url = state
            .public_base_url
            .clone()
            .ok_or_else(|| unavailable("PUBLIC_BASE_URL not configured"))?;
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

fn request_client_hint(headers: &HeaderMap) -> String {
    headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .or_else(|| headers.get("x-real-ip").and_then(|v| v.to_str().ok()))
        .unwrap_or("unknown")
        .to_string()
}

fn detect_burst(scope: &str, key: &str, window_secs: i64, threshold: usize) -> Option<usize> {
    let now = Utc::now().timestamp();
    let lower = now.saturating_sub(window_secs);
    let storage = SECURITY_WINDOW_BUCKETS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut guard = storage.lock().expect("security bucket mutex poisoned");
    let bucket_key = format!("{scope}:{key}");
    let entries = guard.entry(bucket_key).or_default();
    entries.retain(|ts| *ts >= lower);
    entries.push(now);
    let count = entries.len();
    if count == threshold {
        Some(count)
    } else {
        None
    }
}

fn emit_security_alert(state: &AppState, kind: &'static str, payload: serde_json::Value) {
    let Some(url) = state.security_alert_webhook_url.clone() else {
        return;
    };
    tokio::spawn(async move {
        let body = json!({
            "event": "security_alert",
            "kind": kind,
            "timestamp": Utc::now().to_rfc3339(),
            "payload": payload,
        });
        match reqwest::Client::new().post(&url).json(&body).send().await {
            Ok(resp) if resp.status().is_success() => {
                tracing::warn!(target: "backend", kind, status = %resp.status(), "security webhook delivered");
            }
            Ok(resp) => {
                let status = resp.status();
                let body = resp.text().await.unwrap_or_default();
                tracing::warn!(target: "backend", kind, %status, response = %body, "security webhook returned non-success");
            }
            Err(err) => {
                tracing::warn!(target: "backend", kind, error = %err, "security webhook delivery failed");
            }
        }
    });
}

async fn get_withdrawal(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> ApiResult<WithdrawalView> {
    let session = resolve_session_from_headers(&state, &headers).await?;
    let withdrawal = match state.db.fetch_withdrawal(&id).await {
        Ok(wd) => wd,
        Err(sqlx::Error::RowNotFound) => return Err(not_found("withdrawal_not_found")),
        Err(err) => return Err(server_error(err)),
    };

    ensure_session_access(withdrawal.session_id.as_deref(), session.as_ref())?;

    Ok(Json(ApiResponse {
        data: WithdrawalView::new(withdrawal, state.cashu_mint_url.as_deref(), None),
    }))
}

async fn submit_payment_request(
    State(state): State<AppState>,
    Path(id): Path<String>,
    body: Bytes,
) -> ApiResult<PaymentAcceptResponse> {
    let payload: Nut18PaymentPayload = serde_json::from_slice(&body)
        .map_err(|err| invalid_request(format!("invalid payment payload JSON: {err}")))?;
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
            tracing::info!(
                target: "backend",
                withdrawal_id = %withdrawal.id,
                expected_mint = %expected,
                actual_mint = %payload.mint,
                "accepting foreign mint payment; token will be swapped to canonical during redemption"
            );
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
                if let Some(id_value) = map.get_mut("id") {
                    if let serde_json::Value::String(id_str) = id_value {
                        if id_str.len() > 16 {
                            tracing::debug!(
                                target: "backend",
                                withdrawal_id = %withdrawal_id,
                                short_id = %&id_str[..16],
                                full_id = %id_str,
                                "truncating proof keyset id to 8-byte short form"
                            );
                            id_str.truncate(16);
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
    let token_v3: TokenV3 = serde_json::from_str(&token_body).map_err(|err| {
        tracing::warn!(
            target: "backend",
            withdrawal_id = %withdrawal.id,
            error = %err,
            "failed to deserialize payment request token"
        );
        invalid_request("invalid proof payload")
    })?;
    let token_struct = Token::TokenV3(token_v3);
    let encoded = token_struct.to_string();
    let amount = token_struct
        .value()
        .map_err(|_| invalid_request("token references multiple mints"))?
        .to_u64();

    let raw_onchain = match state.onchain_wallet.as_ref() {
        Some(wallet) => {
            let summary = wallet.balance().await.map_err(server_error)?;
            summary.confirmed + summary.trusted_pending
        }
        None => return Err(unavailable("on-chain wallet not configured")),
    };
    let reserved_onchain = state
        .db
        .reserved_onchain_withdrawal_sats()
        .await
        .map_err(server_error)?;
    let available_onchain = raw_onchain.saturating_sub(reserved_onchain);
    let withdrawal_cap = ((available_onchain as f64) * state.single_request_cap_ratio).floor() as u64;
    if withdrawal_cap == 0 || amount > withdrawal_cap {
        tracing::warn!(
            target: "backend",
            withdrawal_id = %withdrawal.id,
            amount_sats = amount,
            raw_onchain,
            reserved_onchain,
            available_onchain,
            withdrawal_cap,
            "rejecting payment request callback due to on-chain reservation pressure"
        );
        return Err(unavailable(
            "on-chain payout capacity is currently exhausted; try again shortly",
        ));
    }

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
            .mint_quote(
                cdk::nuts::PaymentMethod::Known(method),
                Some(Amount::from(req.amount_sats)),
                None,
                None,
            )
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
    let mut quote = {
        let guard = wallet.lock().await;
        guard
            .check_mint_quote_status(&id)
            .await
            .map_err(server_error)?
    };

    // Operator top-ups should be automatic: once a mint quote is paid, mint it into the wallet
    // without requiring a separate manual click.
    if quote.state == MintQuoteState::Paid {
        {
            let guard = wallet.lock().await;
            guard
                .mint(&id, SplitTarget::default(), None)
                .await
                .map_err(server_error)?;
        }
        quote = {
            let guard = wallet.lock().await;
            guard
                .check_mint_quote_status(&id)
                .await
                .map_err(server_error)?
        };
    }
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

async fn list_operator_deposits(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<OperatorListQuery>,
) -> ApiResult<Vec<Deposit>> {
    require_operator_token(&state, &headers)?;

    let states = if query.state.is_empty() {
        DEFAULT_DEPOSIT_STATES.to_vec()
    } else {
        let mut parsed = Vec::with_capacity(query.state.len());
        for raw in query.state.iter() {
            match DepositState::try_from(raw.as_str()) {
                Ok(state) => parsed.push(state),
                Err(_) => {
                    return Err(invalid_request(format!("unknown deposit state '{}'", raw)));
                }
            }
        }
        if parsed.is_empty() {
            DEFAULT_DEPOSIT_STATES.to_vec()
        } else {
            parsed
        }
    };

    let mut deposits = state
        .db
        .list_deposits_by_state(&states)
        .await
        .map_err(server_error)?;
    deposits.sort_by_key(|dep| dep.created_at);

    let limit = query
        .limit
        .unwrap_or(DEFAULT_OPERATOR_WITHDRAWAL_LIMIT)
        .min(DEFAULT_OPERATOR_WITHDRAWAL_LIMIT);
    if deposits.len() > limit {
        deposits.truncate(limit);
    }

    Ok(Json(ApiResponse { data: deposits }))
}

async fn list_operator_withdrawals(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<OperatorListQuery>,
) -> ApiResult<Vec<WithdrawalView>> {
    require_operator_token(&state, &headers)?;

    let states = if query.state.is_empty() {
        DEFAULT_WITHDRAWAL_STATES.to_vec()
    } else {
        let mut parsed = Vec::with_capacity(query.state.len());
        for raw in query.state.iter() {
            match WithdrawalState::try_from(raw.as_str()) {
                Ok(state) => parsed.push(state),
                Err(_) => {
                    return Err(invalid_request(format!(
                        "unknown withdrawal state '{}'",
                        raw
                    )));
                }
            }
        }
        if parsed.is_empty() {
            DEFAULT_WITHDRAWAL_STATES.to_vec()
        } else {
            parsed
        }
    };

    let mut withdrawals = state
        .db
        .list_withdrawals_by_state(&states)
        .await
        .map_err(server_error)?;
    withdrawals.sort_by_key(|wd| wd.created_at);

    let limit = query
        .limit
        .unwrap_or(DEFAULT_OPERATOR_WITHDRAWAL_LIMIT)
        .min(DEFAULT_OPERATOR_WITHDRAWAL_LIMIT);
    if withdrawals.len() > limit {
        withdrawals.truncate(limit);
    }

    let views = withdrawals
        .into_iter()
        .map(|wd| WithdrawalView::new(wd, state.cashu_mint_url.as_deref(), None))
        .collect();

    Ok(Json(ApiResponse { data: views }))
}

async fn operate_deposit(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(req): Json<DepositActionRequest>,
) -> ApiResult<Deposit> {
    require_operator_token(&state, &headers)?;
    let deposit = match state.db.fetch_deposit(&id).await {
        Ok(dep) => dep,
        Err(sqlx::Error::RowNotFound) => return Err(not_found("deposit_not_found")),
        Err(err) => return Err(server_error(err)),
    };

    match req.action {
        DepositActionKind::MarkFulfilled => {
            if !matches!(
                deposit.state,
                DepositState::Delivering | DepositState::Ready
            ) {
                return Err(invalid_request(
                    "only delivering/ready deposits can be fulfilled manually",
                ));
            }
            state
                .db
                .manual_update_deposit_state(&deposit.id, DepositState::Fulfilled, None, true)
                .await
                .map_err(server_error)?;
        }
        DepositActionKind::MarkFailed => {
            if deposit.state == DepositState::Fulfilled {
                return Err(invalid_request(
                    "fulfilled deposits cannot be marked failed",
                ));
            }
            if deposit.state == DepositState::ArchivedByOperator {
                return Err(invalid_request("archived deposits cannot be marked failed"));
            }
            let reason = req
                .note
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
                .unwrap_or_else(|| "marked failed by operator".to_string());
            state
                .db
                .manual_update_deposit_state(&deposit.id, DepositState::Failed, Some(reason), false)
                .await
                .map_err(server_error)?;
        }
        DepositActionKind::Archive => {
            if !matches!(
                deposit.state,
                DepositState::Failed | DepositState::Fulfilled
            ) {
                return Err(invalid_request(
                    "only failed or fulfilled deposits can be archived",
                ));
            }
            state
                .db
                .manual_update_deposit_state(
                    &deposit.id,
                    DepositState::ArchivedByOperator,
                    None,
                    false,
                )
                .await
                .map_err(server_error)?;
        }
    }

    let updated = state.db.fetch_deposit(&id).await.map_err(server_error)?;
    Ok(Json(ApiResponse { data: updated }))
}

async fn operate_withdrawal(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(req): Json<WithdrawalActionRequest>,
) -> ApiResult<WithdrawalView> {
    require_operator_token(&state, &headers)?;
    let withdrawal = match state.db.fetch_withdrawal(&id).await {
        Ok(wd) => wd,
        Err(sqlx::Error::RowNotFound) => return Err(not_found("withdrawal_not_found")),
        Err(err) => return Err(server_error(err)),
    };

    match req.action {
        WithdrawalActionKind::MarkSettled => {
            if matches!(
                withdrawal.state,
                WithdrawalState::Settled | WithdrawalState::Funding
            ) {
                return Err(invalid_request(
                    "withdrawal cannot be marked settled from its current state",
                ));
            }
            let provided_txid = req
                .txid
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_owned);
            let txid = provided_txid.or(withdrawal.txid.clone());
            state
                .db
                .manual_update_withdrawal_state(
                    &withdrawal.id,
                    WithdrawalState::Settled,
                    txid,
                    None,
                    false,
                )
                .await
                .map_err(server_error)?;
        }
        WithdrawalActionKind::MarkFailed => {
            if withdrawal.state == WithdrawalState::Settled {
                return Err(invalid_request(
                    "settled withdrawals cannot be marked failed",
                ));
            }
            let reason = req
                .note
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
                .unwrap_or_else(|| "marked failed by operator".to_string());
            let txid = withdrawal.txid.clone();
            state
                .db
                .manual_update_withdrawal_state(
                    &withdrawal.id,
                    WithdrawalState::Failed,
                    txid,
                    Some(reason),
                    false,
                )
                .await
                .map_err(server_error)?;
        }
        WithdrawalActionKind::Requeue => {
            if !matches!(
                withdrawal.state,
                WithdrawalState::Failed
                    | WithdrawalState::Broadcasting
                    | WithdrawalState::Confirming
            ) {
                return Err(invalid_request(
                    "only failed/broadcasting/confirming withdrawals can be requeued",
                ));
            }
            state
                .db
                .manual_update_withdrawal_state(
                    &withdrawal.id,
                    WithdrawalState::Queued,
                    withdrawal.txid.clone(),
                    None,
                    true,
                )
                .await
                .map_err(server_error)?;
        }
        WithdrawalActionKind::Archive => {
            if !matches!(
                withdrawal.state,
                WithdrawalState::Failed | WithdrawalState::Funding
            ) {
                return Err(invalid_request(
                    "only failed or unfunded withdrawals can be archived",
                ));
            }
            state
                .db
                .manual_update_withdrawal_state(
                    &withdrawal.id,
                    WithdrawalState::ArchivedByOperator,
                    withdrawal.txid.clone(),
                    withdrawal.error.clone(),
                    false,
                )
                .await
                .map_err(server_error)?;
        }
    }

    let updated = state.db.fetch_withdrawal(&id).await.map_err(server_error)?;

    Ok(Json(ApiResponse {
        data: WithdrawalView::new(updated, state.cashu_mint_url.as_deref(), None),
    }))
}

async fn get_ledger_snapshot(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> ApiResult<LedgerSnapshotResponse> {
    require_operator_token(&state, &headers)?;

    let onchain_balance = match state.onchain_wallet.as_ref() {
        Some(wallet) => Some(wallet.balance().await.map_err(server_error)?),
        None => None,
    };

    let cashu_balances = match state.cashu_wallet.as_ref() {
        Some(handle) => {
            let guard = handle.lock().await;
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
            Some((spendable, pending, reserved))
        }
        None => None,
    };

    let deposit_rows = state
        .db
        .ledger_deposit_liabilities()
        .await
        .map_err(server_error)?;
    let withdrawal_rows = state
        .db
        .ledger_withdrawal_liabilities()
        .await
        .map_err(server_error)?;

    let deposits_pending = sum_deposit_states(
        &deposit_rows,
        &[DepositState::Pending, DepositState::Confirming],
    );
    let deposits_minting = sum_deposit_states(
        &deposit_rows,
        &[DepositState::Minting, DepositState::Delivering],
    );
    let deposits_ready = sum_deposit_states(&deposit_rows, &[DepositState::Ready]);

    let withdrawals_funding = sum_withdrawal_states(&withdrawal_rows, &[WithdrawalState::Funding]);
    let withdrawals_queued = sum_withdrawal_states(&withdrawal_rows, &[WithdrawalState::Queued]);
    let withdrawals_broadcasting =
        sum_withdrawal_states(&withdrawal_rows, &[WithdrawalState::Broadcasting]);
    let withdrawals_confirming =
        sum_withdrawal_states(&withdrawal_rows, &[WithdrawalState::Confirming]);

    let (cashu_spendable, cashu_pending, cashu_reserved) = cashu_balances.unwrap_or((0, 0, 0));
    let cashu_assets = CashuAssetBreakdown {
        spendable: cashu_spendable,
        pending: cashu_pending,
        reserved: cashu_reserved,
        available_sats: cashu_spendable,
    };

    let onchain_assets = if let Some(summary) = onchain_balance {
        OnchainAssetBreakdown {
            confirmed: summary.confirmed,
            trusted_pending: summary.trusted_pending,
            untrusted_pending: summary.untrusted_pending,
            immature: summary.immature,
            available_sats: summary.confirmed + summary.trusted_pending,
        }
    } else {
        OnchainAssetBreakdown {
            confirmed: 0,
            trusted_pending: 0,
            untrusted_pending: 0,
            immature: 0,
            available_sats: 0,
        }
    };

    let cashu_liability_total =
        deposits_pending.amount_sats + deposits_minting.amount_sats + deposits_ready.amount_sats;
    let onchain_liability_total = withdrawals_queued.amount_sats
        + withdrawals_broadcasting.amount_sats
        + withdrawals_confirming.amount_sats;

    let cashu_net = cashu_assets.available_sats as i64 - cashu_liability_total as i64;
    let onchain_net = onchain_assets.available_sats as i64 - onchain_liability_total as i64;

    let totals = LedgerTotalsResponse {
        assets_sats: cashu_assets.available_sats + onchain_assets.available_sats,
        liabilities_sats: cashu_liability_total + onchain_liability_total,
        net_sats: cashu_net + onchain_net,
    };

    Ok(Json(ApiResponse {
        data: LedgerSnapshotResponse {
            captured_at: Utc::now().to_rfc3339(),
            float_target_sats: state.float_target_sats,
            cashu: CashuLedgerResponse {
                assets: cashu_assets,
                liabilities: CashuLiabilityBreakdown {
                    awaiting_confirmations: deposits_pending,
                    minting: deposits_minting,
                    ready: deposits_ready,
                    total_sats: cashu_liability_total,
                },
                net_sats: cashu_net,
            },
            onchain: OnchainLedgerResponse {
                assets: onchain_assets,
                liabilities: OnchainLiabilityBreakdown {
                    funding: withdrawals_funding,
                    queued: withdrawals_queued,
                    broadcasting: withdrawals_broadcasting,
                    confirming: withdrawals_confirming,
                    total_sats: onchain_liability_total,
                },
                net_sats: onchain_net,
            },
            totals,
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
        confirmed.to_v3_string()
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

async fn get_operation_mode(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> ApiResult<OperationModeResponse> {
    require_operator_token(&state, &headers)?;
    let mode = state.current_operation_mode().await;
    Ok(Json(ApiResponse {
        data: OperationModeResponse { mode },
    }))
}

async fn get_transaction_counter(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> ApiResult<TransactionCounterResponse> {
    require_operator_token(&state, &headers)?;
    let count = state.db.transaction_counter().await.map_err(server_error)?;
    Ok(Json(ApiResponse {
        data: TransactionCounterResponse { count },
    }))
}

async fn update_operation_mode(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<OperationModeUpdateRequest>,
) -> ApiResult<OperationModeResponse> {
    require_operator_token(&state, &headers)?;
    state
        .set_operation_mode(req.mode)
        .await
        .map_err(server_error)?;
    tracing::info!(target: "backend", mode = %req.mode, "operator mode updated");
    Ok(Json(ApiResponse {
        data: OperationModeResponse { mode: req.mode },
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

fn sum_deposit_states(rows: &[StateLiabilityRow], states: &[DepositState]) -> LiabilityBucket {
    let mut bucket = LiabilityBucket {
        amount_sats: 0,
        count: 0,
    };
    for row in rows {
        if let Ok(state) = DepositState::try_from(row.state.as_str()) {
            if states.iter().any(|target| *target == state) {
                bucket.amount_sats += row.amount_sats;
                bucket.count += row.count;
            }
        }
    }
    bucket
}

fn sum_withdrawal_states(
    rows: &[StateLiabilityRow],
    states: &[WithdrawalState],
) -> LiabilityBucket {
    let mut bucket = LiabilityBucket {
        amount_sats: 0,
        count: 0,
    };
    for row in rows {
        if let Ok(state) = WithdrawalState::try_from(row.state.as_str()) {
            if states.iter().any(|target| *target == state) {
                bucket.amount_sats += row.amount_sats;
                bucket.count += row.count;
            }
        }
    }
    bucket
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
        let client_hint = request_client_hint(headers);
        if let Some(count) = detect_burst(
            "operator401",
            &client_hint,
            OPERATOR_401_WINDOW_SECS,
            OPERATOR_401_THRESHOLD,
        ) {
            emit_security_alert(
                state,
                "operator_auth_401_burst",
                json!({
                    "count": count,
                    "window_seconds": OPERATOR_401_WINDOW_SECS,
                    "threshold": OPERATOR_401_THRESHOLD,
                    "client": client_hint,
                }),
            );
        }
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

async fn resolve_session_from_headers(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<Option<Session>, (StatusCode, Json<ApiError>)> {
    let value = match headers.get(SESSION_HEADER) {
        Some(val) => val,
        None => return Ok(None),
    };

    let token_str = value
        .to_str()
        .map_err(|_| unauthorized_session("session_invalid"))?;
    let normalized = match normalize_session_token(token_str) {
        Some(token) => token,
        None => return Err(unauthorized_session("session_invalid")),
    };

    let now = Utc::now();
    let token_hash = hash_session_token(&normalized);
    let mut session = match state.db.fetch_session_by_token_hash(&token_hash).await {
        Ok(sess) => sess,
        Err(sqlx::Error::RowNotFound) => return Err(unauthorized_session("session_invalid")),
        Err(err) => return Err(server_error(err)),
    };

    if session.expires_at < now {
        return Err(unauthorized_session("session_expired"));
    }

    let new_expiry = now + state.session_ttl;
    if let Err(err) = state.db.touch_session(&session.id, now, new_expiry).await {
        tracing::warn!(
            target: "backend",
            error = %err,
            session_id = %session.id,
            "failed to refresh session timestamps"
        );
    } else {
        session.last_seen_at = now;
        session.expires_at = new_expiry;
    }

    Ok(Some(session))
}

fn generate_session_token() -> (String, String) {
    let mut bytes = [0u8; SESSION_TOKEN_BYTES];
    OsRng.fill_bytes(&mut bytes);
    let payload = hex::encode(bytes);
    let token = format!("{}{}", SESSION_TOKEN_PREFIX, payload);
    let claim_code = claim_code_from_token_payload(&payload);
    (token, claim_code)
}

fn claim_code_from_token_payload(payload: &str) -> String {
    let upper = payload.to_uppercase();
    upper
        .chars()
        .collect::<Vec<_>>()
        .chunks(SESSION_CLAIM_GROUP_LEN)
        .map(|chunk| chunk.iter().collect::<String>())
        .collect::<Vec<_>>()
        .join("-")
}

fn normalize_session_token(value: &str) -> Option<String> {
    let trimmed = value.trim();
    let payload = trimmed.strip_prefix(SESSION_TOKEN_PREFIX)?;
    if payload.len() != SESSION_TOKEN_HEX_LEN {
        return None;
    }
    if !payload.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    Some(format!(
        "{}{}",
        SESSION_TOKEN_PREFIX,
        payload.to_ascii_lowercase()
    ))
}

fn normalize_claim_code(input: &str) -> Option<String> {
    let trimmed = input.trim();
    let without_prefix = trimmed
        .strip_prefix(SESSION_TOKEN_PREFIX)
        .unwrap_or(trimmed);
    let mut filtered = String::with_capacity(SESSION_TOKEN_HEX_LEN);
    for ch in without_prefix.chars() {
        if ch == '-' || ch.is_whitespace() {
            continue;
        }
        if ch.is_ascii_hexdigit() {
            filtered.push(ch.to_ascii_lowercase());
        } else {
            return None;
        }
    }
    if filtered.len() != SESSION_TOKEN_HEX_LEN {
        return None;
    }
    Some(filtered)
}

fn hash_session_token(token: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(token.as_bytes());
    hex::encode(hasher.finalize())
}

fn ensure_session_access(
    owner_session: Option<&str>,
    current: Option<&Session>,
) -> Result<(), (StatusCode, Json<ApiError>)> {
    if let Some(expected) = owner_session {
        let Some(active) = current else {
            return Err(unauthorized_session("session_required"));
        };
        if active.id != expected {
            return Err(unauthorized_session("session_mismatch"));
        }
    }
    Ok(())
}

fn unauthorized_session(code: &'static str) -> (StatusCode, Json<ApiError>) {
    (
        StatusCode::UNAUTHORIZED,
        Json(ApiError {
            code,
            message: "invalid or expired session token".into(),
        }),
    )
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

fn mode_blocked(mode: OperationMode, resource: &str) -> (StatusCode, Json<ApiError>) {
    let message = match mode {
        OperationMode::Drain => {
            format!("Shuestand is draining; new {resource} are temporarily disabled")
        }
        OperationMode::Halt => {
            format!("Shuestand is paused; new {resource} are temporarily disabled")
        }
        OperationMode::Normal => unreachable!("mode_blocked should not be called in normal mode"),
    };
    unavailable(&message)
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
