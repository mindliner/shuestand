use axum::{
    Json, Router,
    extract::{Path, State},
    http::{
        HeaderMap, HeaderValue, StatusCode,
        header::{AUTHORIZATION, CONTENT_TYPE},
    },
    response::IntoResponse,
    routing::{get, post},
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::{sync::Arc, time::Instant};
use uuid::Uuid;

use crate::AppState;
use backend::cashu::{TokenMintError, token_mint_url};
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

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(healthcheck))
        .route("/metrics", get(export_metrics))
        .route("/api/v1/deposits", post(create_deposit))
        .route("/api/v1/deposits/:id", get(get_deposit))
        .route("/api/v1/withdrawals", post(request_withdrawal))
        .route("/api/v1/withdrawals/:id", get(get_withdrawal))
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
    token: String,
    delivery_address: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    max_fee_sats: Option<u64>,
}

#[derive(Serialize)]
struct WithdrawalView {
    #[serde(flatten)]
    withdrawal: Withdrawal,
    #[serde(skip_serializing_if = "Option::is_none")]
    source_mint_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    is_foreign_mint: Option<bool>,
}

impl WithdrawalView {
    fn new(
        withdrawal: Withdrawal,
        canonical_mint: Option<&str>,
        known_mint: Option<String>,
    ) -> Self {
        let source_mint_url = if let Some(url) = known_mint {
            Some(url)
        } else {
            match token_mint_url(&withdrawal.token) {
                Ok(url) => Some(url),
                Err(err) => {
                    tracing::warn!(target: "backend", error = %err, id = %withdrawal.id, "failed to parse mint for withdrawal");
                    None
                }
            }
        };
        let is_foreign_mint = match (canonical_mint, source_mint_url.as_deref()) {
            (Some(expected), Some(actual)) => Some(actual != expected),
            _ => None,
        };
        Self {
            withdrawal,
            source_mint_url,
            is_foreign_mint,
        }
    }
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
    Json(req): Json<WithdrawalRequest>,
) -> ApiResult<WithdrawalView> {
    let token_raw = req.token.trim();
    if token_raw.is_empty() {
        return Err(invalid_request("token is required"));
    }
    if req.delivery_address.trim().is_empty() {
        return Err(invalid_request("delivery_address is required"));
    }

    let mint_url = token_mint_url(token_raw).map_err(map_token_error)?;

    let id = format!("wd_{}", Uuid::new_v4());
    let now = Utc::now();

    let withdrawal = Withdrawal {
        id: id.clone(),
        state: WithdrawalState::Queued,
        delivery_address: req.delivery_address,
        max_fee_sats: req.max_fee_sats,
        token_value_sats: None,
        token: token_raw.to_string(),
        txid: None,
        error: None,
        last_attempt_at: None,
        attempt_count: 0,
        created_at: now,
        updated_at: now,
        token_consumed: false,
        swap_fee_sats: None,
    };

    state
        .db
        .insert_withdrawal(&withdrawal)
        .await
        .map_err(server_error)?;

    Ok(Json(ApiResponse {
        data: WithdrawalView::new(withdrawal, state.cashu_mint_url.as_deref(), Some(mint_url)),
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
    Ok(Json(ApiResponse {
        data: FloatStatusResponse {
            target_sats: state.float_target_sats,
            min_ratio: state.float_min_ratio,
            max_ratio: state.float_max_ratio,
            onchain: WalletFloatStatusPayload::from(&snapshot.onchain),
            cashu: WalletFloatStatusPayload::from(&snapshot.cashu),
        },
    }))
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

fn invalid_request(message: &str) -> (StatusCode, Json<ApiError>) {
    (
        StatusCode::BAD_REQUEST,
        Json(ApiError {
            code: "validation_error",
            message: message.to_string(),
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
