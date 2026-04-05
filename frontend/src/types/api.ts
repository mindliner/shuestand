export type DepositState =
  | 'pending'
  | 'confirming'
  | 'minting'
  | 'delivering'
  | 'ready'
  | 'fulfilled'
  | 'failed'
  | 'archived_by_operator'

export interface Deposit {
  id: string
  amount_sats: number
  state: DepositState
  address: string
  target_confirmations: number
  delivery_hint?: string | null
  metadata?: Record<string, unknown> | null
  txid?: string | null
  confirmations: number
  last_checked_at?: string | null
  last_event?: string | null
  token?: string | null
  token_qr?: string | null
  created_at?: string | null
  updated_at?: string | null
  delivery_error?: string | null
  session_id?: string | null
}

export type WithdrawalState =
  | 'funding'
  | 'queued'
  | 'broadcasting'
  | 'confirming'
  | 'settled'
  | 'failed'
  | 'archived_by_operator'

export interface WithdrawalPaymentRequest {
  creq: string
  expires_at?: string | null
  fulfilled_at?: string | null
}

export interface Withdrawal {
  id: string
  state: WithdrawalState
  delivery_address: string
  max_fee_sats?: number | null
  requested_amount_sats?: number | null
  token_value_sats?: number | null
  txid?: string | null
   error?: string | null
  fee_paid_sats?: number | null
  source_mint_url?: string | null
  is_foreign_mint?: boolean | null
  token_consumed: boolean
  session_id?: string | null
  swap_fee_sats?: number | null
  payment_request?: WithdrawalPaymentRequest | null
  last_attempt_at?: string | null
  attempt_count: number
  created_at?: string | null
  updated_at?: string | null
}

export interface OperatorWithdrawalListParams {
  states?: WithdrawalState[]
  limit?: number
}

export interface OperatorWithdrawalActionRequest {
  action: 'mark_settled' | 'mark_failed' | 'requeue' | 'archive'
  note?: string
  txid?: string
}

export interface OperatorDepositActionRequest {
  action: 'mark_fulfilled' | 'mark_failed' | 'archive'
  note?: string
}

export interface ApiResponse<T> {
  data: T
}

export interface ApiError {
  code: string
  message: string
}

export interface CreateDepositRequest {
  amount_sats: number
  delivery_hint?: string
  metadata?: Record<string, unknown>
}

export interface DepositCreationResponse {
  deposit: Deposit
  pickup_token: string
}

export interface SessionStartResponse {
  session_id: string
  token: string
  claim_code: string
  expires_at: string
}

export interface DepositPickupResponse {
  token: string
}

export interface CreateWithdrawalRequest {
  amount_sats: number
  delivery_address: string
  token?: string
  max_fee_sats?: number
  create_payment_request?: boolean
}

export interface WalletBalanceResponse {
  confirmed: number
  trusted_pending: number
  untrusted_pending: number
  immature: number
}

export interface WalletSendRequest {
  address: string
  amount_sats: number
  fee_rate_vb: number
}

export interface WalletSendResponse {
  txid: string
}

export interface WalletTopUpResponse {
  address: string
  bip21_uri: string
}

export type CashuInvoiceMethod = 'bolt11' | 'bolt12' | 'custom'

export interface CashuInvoiceStatus {
  quote_id: string
  amount_sats: number
  amount_paid_sats: number
  amount_issued_sats: number
  request: string
  method: CashuInvoiceMethod
  state: string
  expires_at: number
}

export interface CashuInvoiceRequest {
  amount_sats: number
  bolt12?: boolean
}

export interface CashuMintResponse {
  minted: boolean
  amount_sats: number
}

export interface CashuWalletBalanceResponse {
  spendable: number
  pending: number
  reserved: number
}

export interface CashuSendRequest {
  amount_sats: number
}

export interface CashuSendResponse {
  token: string
}

export type FloatState = 'ok' | 'low' | 'high' | 'unknown'

export interface WalletFloatStatus {
  balance_sats: number
  ratio: number
  state: FloatState
  updated_at?: string | null
}

export interface FloatStatusResponse {
  target_sats: number
  min_ratio: number
  max_ratio: number
  onchain: WalletFloatStatus
  cashu: WalletFloatStatus
  total_balance_sats?: number
  drift_sats?: number
}

export interface PublicConfigResponse {
  withdrawal_min_sats: number
  deposit_min_sats: number
  deposit_target_confirmations: number
  float_target_sats: number
  float_min_ratio: number
  float_max_ratio: number
  single_request_cap_ratio: number
  deposit_flow_enabled: boolean
  deposit_flow_reason?: string | null
  cashu_mint_url?: string | null
}

export type OperationMode = 'normal' | 'drain' | 'halt'

export interface OperationModeResponse {
  mode: OperationMode
}

export interface LedgerSnapshotResponse {
  captured_at: string
  float_target_sats: number
  cashu: CashuLedgerResponse
  onchain: OnchainLedgerResponse
  totals: LedgerTotals
}

export interface LedgerTotals {
  assets_sats: number
  liabilities_sats: number
  net_sats: number
}

export interface CashuLedgerResponse {
  assets: CashuAssetBreakdown
  liabilities: CashuLiabilityBreakdown
  net_sats: number
}

export interface CashuAssetBreakdown {
  spendable: number
  pending: number
  reserved: number
  available_sats: number
}

export interface LiabilityBucket {
  amount_sats: number
  count: number
}

export interface CashuLiabilityBreakdown {
  awaiting_confirmations: LiabilityBucket
  minting: LiabilityBucket
  ready: LiabilityBucket
  total_sats: number
}

export interface OnchainLedgerResponse {
  assets: OnchainAssetBreakdown
  liabilities: OnchainLiabilityBreakdown
  net_sats: number
}

export interface OnchainAssetBreakdown {
  confirmed: number
  trusted_pending: number
  untrusted_pending: number
  immature: number
  available_sats: number
}

export interface OnchainLiabilityBreakdown {
  funding: LiabilityBucket
  queued: LiabilityBucket
  broadcasting: LiabilityBucket
  confirming: LiabilityBucket
  total_sats: number
}
