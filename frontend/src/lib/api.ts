import { config } from '../config'
import type {
  ApiError,
  ApiResponse,
  CreateDepositRequest,
  CreateWithdrawalRequest,
  Deposit,
  DepositCreationResponse,
  SessionStartResponse,
  DepositPickupResponse,
  WalletBalanceResponse,
  WalletSendRequest,
  WalletSendResponse,
  WalletTopUpResponse,
  CashuInvoiceRequest,
  CashuInvoiceStatus,
  CashuMintResponse,
  CashuWalletBalanceResponse,
  CashuSendRequest,
  CashuSendResponse,
  FloatStatusResponse,
  LedgerSnapshotResponse,
  Withdrawal,
  OperatorWithdrawalActionRequest,
  OperatorWithdrawalListParams,
  OperatorDepositActionRequest,
} from '../types/api'

const JSON_HEADERS = {
  'Content-Type': 'application/json',
}

const jsonHeaders = (token?: string) => ({
  ...JSON_HEADERS,
  ...(token ? { Authorization: `Bearer ${token}` } : {}),
})

const sessionHeaders = (token?: string) => ({
  ...JSON_HEADERS,
  ...(token ? { 'X-Shuestand-Session': token } : {}),
})

export class ApiClientError extends Error {
  status: number
  code?: string
  details?: unknown

  constructor(message: string, status: number, code?: string, details?: unknown) {
    super(message)
    this.name = 'ApiClientError'
    this.status = status
    this.code = code
    this.details = details
  }
}

async function request<T>(
  path: string,
  init?: RequestInit
): Promise<T> {
  const res = await fetch(`${config.apiBase}${path}`, init)
  const payload = await parseJson(res)

  if (!res.ok) {
    const apiError = normalizeApiError(payload)
    throw new ApiClientError(
      apiError?.message ?? `Request failed with status ${res.status}`,
      res.status,
      apiError?.code,
      payload
    )
  }

  return (payload as ApiResponse<T>).data ?? (payload as T)
}

async function parseJson(res: Response) {
  const text = await res.text()
  if (!text) {
    return null
  }
  try {
    return JSON.parse(text)
  } catch (err) {
    throw new ApiClientError(
      'Malformed JSON response from backend',
      res.status,
      'invalid_json',
      { cause: err }
    )
  }
}

function normalizeApiError(payload: unknown): ApiError | undefined {
  if (!payload || typeof payload !== 'object') {
    return undefined
  }
  const maybe = payload as { error?: ApiError; code?: unknown; message?: unknown }
  if (maybe.error && typeof maybe.error.message === 'string') {
    return maybe.error
  }
  if (typeof maybe.code === 'string' && typeof maybe.message === 'string') {
    return { code: maybe.code as string, message: maybe.message as string }
  }
  return undefined
}

export function createDeposit(
  payload: CreateDepositRequest,
  sessionToken?: string,
): Promise<DepositCreationResponse> {
  return request<DepositCreationResponse>('/api/v1/deposits', {
    method: 'POST',
    headers: sessionHeaders(sessionToken),
    body: JSON.stringify(payload),
  })
}

export function createWithdrawal(
  payload: CreateWithdrawalRequest,
  sessionToken?: string,
): Promise<Withdrawal> {
  return request<Withdrawal>('/api/v1/withdrawals', {
    method: 'POST',
    headers: sessionHeaders(sessionToken),
    body: JSON.stringify(payload),
  })
}

export function getDeposit(id: string, sessionToken?: string): Promise<Deposit> {
  return request<Deposit>(`/api/v1/deposits/${encodeURIComponent(id)}`, {
    headers: sessionHeaders(sessionToken),
  })
}

export function pickupDeposit(
  id: string,
  pickupToken: string,
  sessionToken?: string,
): Promise<DepositPickupResponse> {
  return request<DepositPickupResponse>(`/api/v1/deposits/${encodeURIComponent(id)}/pickup`, {
    method: 'POST',
    headers: sessionHeaders(sessionToken),
    body: JSON.stringify({ pickup_token: pickupToken }),
  })
}

export function getWithdrawal(id: string, sessionToken?: string): Promise<Withdrawal> {
  return request<Withdrawal>(`/api/v1/withdrawals/${encodeURIComponent(id)}`, {
    headers: sessionHeaders(sessionToken),
  })
}

export function startSession(): Promise<SessionStartResponse> {
  return request<SessionStartResponse>('/api/v1/sessions', {
    method: 'POST',
    headers: JSON_HEADERS,
  })
}

export function resumeSession(claimCode: string): Promise<SessionStartResponse> {
  return request<SessionStartResponse>('/api/v1/sessions/resume', {
    method: 'POST',
    headers: JSON_HEADERS,
    body: JSON.stringify({ claim_code: claimCode }),
  })
}

export function getWalletBalance(token: string): Promise<WalletBalanceResponse> {
  return request<WalletBalanceResponse>('/api/v1/wallet/balance', {
    headers: jsonHeaders(token),
  })
}

export function syncWallet(token: string): Promise<WalletBalanceResponse> {
  return request<WalletBalanceResponse>('/api/v1/wallet/sync', {
    method: 'POST',
    headers: jsonHeaders(token),
  })
}

export function sendWalletPayment(
  token: string,
  payload: WalletSendRequest,
): Promise<WalletSendResponse> {
  return request<WalletSendResponse>('/api/v1/wallet/send', {
    method: 'POST',
    headers: jsonHeaders(token),
    body: JSON.stringify(payload),
  })
}

export function getWalletTopup(token: string): Promise<WalletTopUpResponse> {
  return request<WalletTopUpResponse>('/api/v1/wallet/topup', {
    headers: jsonHeaders(token),
  })
}

export function createCashuInvoice(
  token: string,
  payload: CashuInvoiceRequest,
): Promise<CashuInvoiceStatus> {
  return request<CashuInvoiceStatus>('/api/v1/cashu/invoices', {
    method: 'POST',
    headers: jsonHeaders(token),
    body: JSON.stringify(payload),
  })
}

export function getCashuInvoice(
  token: string,
  quoteId: string,
): Promise<CashuInvoiceStatus> {
  return request<CashuInvoiceStatus>(`/api/v1/cashu/invoices/${encodeURIComponent(quoteId)}`, {
    headers: jsonHeaders(token),
  })
}

export function mintCashuInvoice(
  token: string,
  quoteId: string,
): Promise<CashuMintResponse> {
  return request<CashuMintResponse>(`/api/v1/cashu/invoices/${encodeURIComponent(quoteId)}/mint`, {
    method: 'POST',
    headers: jsonHeaders(token),
  })
}

export function getCashuWalletBalance(
  token: string,
): Promise<CashuWalletBalanceResponse> {
  return request<CashuWalletBalanceResponse>('/api/v1/cashu/wallet/balance', {
    headers: jsonHeaders(token),
  })
}

export function sendCashuToken(
  token: string,
  payload: CashuSendRequest,
): Promise<CashuSendResponse> {
  return request<CashuSendResponse>('/api/v1/cashu/send', {
    method: 'POST',
    headers: jsonHeaders(token),
    body: JSON.stringify(payload),
  })
}

export function getLedgerSnapshot(token: string): Promise<LedgerSnapshotResponse> {
  return request<LedgerSnapshotResponse>('/api/v1/operator/ledger', {
    headers: jsonHeaders(token),
  })
}

export function getFloatStatus(token: string): Promise<FloatStatusResponse> {
  return request<FloatStatusResponse>('/api/v1/float/status', {
    headers: jsonHeaders(token),
  })
}


export function listOperatorWithdrawals(
  token: string,
  params?: OperatorWithdrawalListParams,
): Promise<Withdrawal[]> {
  const search = new URLSearchParams()
  params?.states?.forEach((state) => {
    search.append('state', state)
  })
  if (typeof params?.limit === 'number') {
    search.set('limit', String(params.limit))
  }
  const qs = search.toString()
  const suffix = qs ? `?${qs}` : ''
  return request<Withdrawal[]>(`/api/v1/operator/withdrawals${suffix}`, {
    headers: jsonHeaders(token),
  })
}

export function operateWithdrawal(
  token: string,
  id: string,
  payload: OperatorWithdrawalActionRequest,
): Promise<Withdrawal> {
  return request<Withdrawal>(
    `/api/v1/operator/withdrawals/${encodeURIComponent(id)}/actions`,
    {
      method: 'POST',
      headers: jsonHeaders(token),
      body: JSON.stringify(payload),
    },
  )
}

export function listOperatorDeposits(
  token: string,
  params?: OperatorWithdrawalListParams,
): Promise<Deposit[]> {
  const search = new URLSearchParams()
  params?.states?.forEach((state) => {
    search.append('state', state)
  })
  if (typeof params?.limit === 'number') {
    search.set('limit', String(params.limit))
  }
  const qs = search.toString()
  const suffix = qs ? `?${qs}` : ''
  return request<Deposit[]>(`/api/v1/operator/deposits${suffix}`, {
    headers: jsonHeaders(token),
  })
}

export function operateDeposit(
  token: string,
  id: string,
  payload: OperatorDepositActionRequest,
): Promise<Deposit> {
  return request<Deposit>(`/api/v1/operator/deposits/${encodeURIComponent(id)}/actions`, {
    method: 'POST',
    headers: jsonHeaders(token),
    body: JSON.stringify(payload),
  })
}
