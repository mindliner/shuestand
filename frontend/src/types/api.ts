export type DepositState =
  | 'pending'
  | 'confirming'
  | 'minting'
  | 'delivering'
  | 'ready'
  | 'fulfilled'
  | 'failed'

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
}

export type WithdrawalState =
  | 'funding'
  | 'queued'
  | 'broadcasting'
  | 'confirming'
  | 'settled'
  | 'failed'

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
  fee_paid_sats?: number | null
  source_mint_url?: string | null
  is_foreign_mint?: boolean | null
  token_consumed: boolean
  swap_fee_sats?: number | null
  payment_request?: WithdrawalPaymentRequest | null
  last_attempt_at?: string | null
  attempt_count: number
  created_at?: string | null
  updated_at?: string | null
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
