import type { FormEvent } from 'react'
import { useEffect, useRef, useState } from 'react'
import { useNavigate } from 'react-router-dom'

const STORAGE_KEYS = {
  session: 'shuestand.session',
  deposits: 'shuestand.deposits',
  withdrawals: 'shuestand.withdrawals',
  legacyDeposit: 'shuestand.latestDepositId',
  legacyWithdrawal: 'shuestand.latestWithdrawalId',
  deliveryAddress: 'shuestand.latestDeliveryAddress',
  archives: 'shuestand.archives',
}
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import './App.css'
import { config } from './config'
import { copyTextWithFallback } from './lib/clipboard'
import { detectTokenMint } from './lib/cashu'
import { isValidBitcoinAddress } from './lib/bitcoin'
import type { Theme } from './lib/theme'
import type {
  CreateWithdrawalRequest,
  SessionStartResponse,
  Deposit,
  Withdrawal,
  OperationMode,
} from './types/api'
import {
  ApiClientError,
  createDeposit,
  createWithdrawal,
  getPublicConfig,
  getDeposit,
  getWithdrawal,
  pickupDeposit,
  startSession,
  resumeSession,
} from './lib/api'
import {
  DepositStatusCard,
  WithdrawalStatusCard,
} from './components/KioskStatusCards'
import { AppVersion } from './components/AppVersion'

type Flow = 'deposit' | 'withdrawal'
type KioskAppProps = {
  theme: Theme
  onThemeSelect: (mode: Theme) => void
}
type TokenMintInfo =
  | { mintUrl: string; isForeign: boolean; amount: number }
  | { error: string }
type StoredDeposit = { id: string; pickupToken?: string | null }
type StoredWithdrawal = { id: string }
type SessionInfo = {
  id: string
  token: string
  claimCode: string
  expiresAt: string
}

type ArchivedEntry = {
  id: string
  amount: number
  kind: 'deposit' | 'withdrawal'
  archivedAt: string
}

const DEFAULT_DEPOSIT_AMOUNT = config.depositMinSats.toString()
const DEFAULT_WITHDRAWAL_AMOUNT = config.withdrawalMinSats.toString()
const STATUS_REFRESH_MS = 5000
const MAX_ARCHIVED_ENTRIES = 20
const DEFAULT_DEPOSIT_MAX_SATS = 2_000_000
const DEPOSIT_SLIDER_STEP_SATS = 1_000

const formatSats = (value: number) => value.toLocaleString('en-US')

const normalizeError = (err: unknown): Error | null => {
  if (!err) return null
  if (
    err instanceof ApiClientError &&
    err.status === 429 &&
    (err.code === 'non_json_error' || /too many requests/i.test(err.message))
  ) {
    return new Error('Too many requests for a moment. Please tap Reveal token again in 1–2 seconds.')
  }
  return err instanceof Error ? err : new Error(String(err))
}

export function KioskApp({ theme, onThemeSelect }: KioskAppProps) {
  const [flow, setFlow] = useState<Flow>('deposit')
  const [depositAmount, setDepositAmount] = useState(DEFAULT_DEPOSIT_AMOUNT)
  const [withdrawalAmount, setWithdrawalAmount] = useState(DEFAULT_WITHDRAWAL_AMOUNT)
  const [withdrawalMethod, setWithdrawalMethod] = useState<'token' | 'payment_request'>(
    'token'
  )
  const [token, setToken] = useState('')
  const [tokenMintInfo, setTokenMintInfo] = useState<TokenMintInfo | null>(null)
  const [deliveryAddress, setDeliveryAddress] = useState('')
  const [isSubmitting, setSubmitting] = useState(false)
  const [message, setMessage] = useState<string | null>(null)
  const [deposits, setDeposits] = useState<StoredDeposit[]>([])
  const [selectedDepositId, setSelectedDepositId] = useState<string | null>(null)
  const [withdrawals, setWithdrawals] = useState<StoredWithdrawal[]>([])
  const [selectedWithdrawalId, setSelectedWithdrawalId] = useState<string | null>(null)
  const [archivedEntries, setArchivedEntries] = useState<ArchivedEntry[]>([])
  const [revealedTokens, setRevealedTokens] = useState<Record<string, string>>({})
  const [session, setSession] = useState<SessionInfo | null>(null)
  const [sessionBusy, setSessionBusy] = useState(false)
  const [resumeCode, setResumeCode] = useState('')
  const [resumeFlowHint, setResumeFlowHint] = useState(false)
  const [sessionHydrationTick, setSessionHydrationTick] = useState(0)
  const [floatingNotice, setFloatingNotice] = useState<string | null>(null)
  const floatingNoticeTimer = useRef<number | null>(null)
  const [limits, setLimits] = useState(() => ({
    withdrawalMinSats: config.withdrawalMinSats,
    depositMinSats: config.depositMinSats,
    depositMaxSats: DEFAULT_DEPOSIT_MAX_SATS,
    pendingDepositTtlSecs: 600,
    depositFlowEnabled: true,
    depositFlowReason: null as string | null,
    operationMode: 'normal' as OperationMode,
    cashuMintUrl: config.cashuMintUrl ? config.cashuMintUrl.trim() : '',
  }))
  const navigate = useNavigate()

  const routeToSupport = (reason: string) => {
    const query = new URLSearchParams()
    query.set('reason', reason)
    navigate(`/support?${query.toString()}`)
  }

  const shouldEscalateToSupport = (err: unknown, context: 'request' | 'pickup') => {
    if (!(err instanceof ApiClientError)) {
      return false
    }

    if (context === 'pickup') {
      // Pickup can fail transiently (race while status refresh catches up, replay/conflict, etc.).
      // Keep the user in-flow so they can immediately retry instead of forcing support.
      if (err.code === 'deposit_not_ready_for_pickup' || err.status < 500) {
        return false
      }
    }

    return err.status >= 500 || err.code === 'non_json_error'
  }

  const showFloatingNotice = (text: string) => {
    setFloatingNotice(text)
    if (typeof window === 'undefined') {
      return
    }
    if (floatingNoticeTimer.current) {
      window.clearTimeout(floatingNoticeTimer.current)
    }
    floatingNoticeTimer.current = window.setTimeout(() => {
      setFloatingNotice(null)
      floatingNoticeTimer.current = null
    }, 5000)
  }

  useEffect(() => {
    return () => {
      if (floatingNoticeTimer.current && typeof window !== 'undefined') {
        window.clearTimeout(floatingNoticeTimer.current)
      }
    }
  }, [])

  const scopedKey = (base: string, sessionId: string) => `${base}.${sessionId}`

  const persistDeposits = (entries: StoredDeposit[], sessionId?: string) => {
    if (typeof window === 'undefined' || !sessionId) {
      return
    }
    window.localStorage.setItem(scopedKey(STORAGE_KEYS.deposits, sessionId), JSON.stringify(entries))
  }

  const persistWithdrawals = (entries: StoredWithdrawal[], sessionId?: string) => {
    if (typeof window === 'undefined' || !sessionId) {
      return
    }
    window.localStorage.setItem(scopedKey(STORAGE_KEYS.withdrawals, sessionId), JSON.stringify(entries))
  }

  const persistArchived = (entries: ArchivedEntry[], sessionId?: string) => {
    if (typeof window === 'undefined' || !sessionId) {
      return
    }
    window.localStorage.setItem(scopedKey(STORAGE_KEYS.archives, sessionId), JSON.stringify(entries))
  }

  const storeSessionInfo = (info: SessionInfo | null) => {
    if (typeof window === 'undefined') {
      return
    }
    if (info) {
      window.localStorage.setItem(STORAGE_KEYS.session, JSON.stringify(info))
    } else {
      window.localStorage.removeItem(STORAGE_KEYS.session)
    }
  }

  const clearSessionCaches = (sessionId?: string) => {
    if (typeof window === 'undefined' || !sessionId) {
      return
    }
    window.localStorage.removeItem(scopedKey(STORAGE_KEYS.deposits, sessionId))
    window.localStorage.removeItem(scopedKey(STORAGE_KEYS.withdrawals, sessionId))
    window.localStorage.removeItem(scopedKey(STORAGE_KEYS.archives, sessionId))
  }

  const resetTrackingState = () => {
    setDeposits([])
    setSelectedDepositId(null)
    setWithdrawals([])
    setSelectedWithdrawalId(null)
    setArchivedEntries([])
    setRevealedTokens({})
  }

  const applySessionPayload = (payload: SessionStartResponse, opts?: { resumed?: boolean }) => {
    const info: SessionInfo = {
      id: payload.session_id,
      token: payload.token,
      claimCode: payload.claim_code,
      expiresAt: payload.expires_at,
    }
    resetTrackingState()
    setSession(info)
    storeSessionInfo(info)
    setResumeFlowHint(Boolean(opts?.resumed))
  }

  const dropSession = (notice?: string) => {
    if (session?.id) {
      clearSessionCaches(session.id)
    }
    storeSessionInfo(null)
    setSession(null)
    setResumeFlowHint(false)
    setSessionHydrationTick(0)
    resetTrackingState()
    setResumeCode('')
    if (notice) {
      setMessage(notice)
    }
  }

  const SESSION_ERROR_CODES = new Set(['session_invalid', 'session_expired', 'session_required'])

  const maybeHandleSessionAuthError = (err: unknown) => {
    if (err instanceof ApiClientError && err.code && SESSION_ERROR_CODES.has(err.code)) {
      dropSession('Session expired or invalid. Start a new one to keep working.')
      return true
    }
    return false
  }

  const queryClient = useQueryClient()
  const sessionToken = session?.token ?? null
  const displayedDepositMaxSats = Math.max(
    limits.depositMinSats,
    Math.floor(limits.depositMaxSats / DEPOSIT_SLIDER_STEP_SATS) * DEPOSIT_SLIDER_STEP_SATS
  )

  useEffect(() => {
    let cancelled = false
    const loadRuntimeConfig = async () => {
      try {
        const runtime = await getPublicConfig()
        if (cancelled || !runtime) {
          return
        }
        const min = Number(runtime.withdrawal_min_sats)
        const resolvedMin = Number.isFinite(min) && min > 0 ? min : undefined
        const depositMin = Number(runtime.deposit_min_sats)
        const resolvedDepositMin =
          Number.isFinite(depositMin) && depositMin > 0 ? depositMin : undefined
        const pendingTtl = Number(runtime.pending_deposit_ttl_secs)
        const resolvedPendingTtl =
          Number.isFinite(pendingTtl) && pendingTtl > 0 ? pendingTtl : undefined
        const depositMax = Number(runtime.deposit_max_sats)
        const resolvedDepositMax =
          Number.isFinite(depositMax) && depositMax > 0 ? depositMax : undefined
        const depositFlowEnabled = runtime.deposit_flow_enabled !== false
        const depositFlowReason = runtime.deposit_flow_reason ?? null
        const operationMode = runtime.operation_mode ?? 'normal'
        const runtimeMint =
          typeof runtime.cashu_mint_url === 'string' ? runtime.cashu_mint_url.trim() : ''
        setLimits((current) => {
          const next = {
            withdrawalMinSats: resolvedMin ?? current.withdrawalMinSats,
            depositMinSats: resolvedDepositMin ?? current.depositMinSats,
            depositMaxSats: resolvedDepositMax ?? current.depositMaxSats,
            pendingDepositTtlSecs: resolvedPendingTtl ?? current.pendingDepositTtlSecs,
            depositFlowEnabled,
            depositFlowReason,
            operationMode,
            cashuMintUrl: runtimeMint,
          }
          if (
            next.withdrawalMinSats === current.withdrawalMinSats &&
            next.depositMinSats === current.depositMinSats &&
            next.pendingDepositTtlSecs === current.pendingDepositTtlSecs &&
            next.depositMaxSats === current.depositMaxSats &&
            next.depositFlowEnabled === current.depositFlowEnabled &&
            next.depositFlowReason === current.depositFlowReason &&
            next.operationMode === current.operationMode &&
            next.cashuMintUrl === current.cashuMintUrl
          ) {
            return current
          }
          return next
        })
        if (!depositFlowEnabled) {
          setFlow((current) => (current === 'deposit' ? 'withdrawal' : current))
        }
      } catch (err) {
        console.warn('Failed to load runtime config', err)
      }
    }

    loadRuntimeConfig()
    return () => {
      cancelled = true
    }
  }, [])

  useEffect(() => {
    setWithdrawalAmount((current) => {
      const numeric = Number(current)
      if (!Number.isFinite(numeric) || numeric >= limits.withdrawalMinSats) {
        return current
      }
      return limits.withdrawalMinSats.toString()
    })
  }, [limits.withdrawalMinSats])

  useEffect(() => {
    setDepositAmount((current) => {
      const numeric = Number(current)
      if (!Number.isFinite(numeric)) {
        return limits.depositMinSats.toString()
      }
      if (numeric < limits.depositMinSats) {
        return limits.depositMinSats.toString()
      }
      if (numeric > limits.depositMaxSats) {
        return limits.depositMaxSats.toString()
      }
      return current
    })
  }, [limits.depositMinSats, limits.depositMaxSats])

  useEffect(() => {
    if (typeof window === 'undefined') {
      return
    }
    const rawSession = window.localStorage.getItem(STORAGE_KEYS.session)
    if (rawSession) {
      try {
        const parsed = JSON.parse(rawSession)
        if (parsed && typeof parsed === 'object' && typeof parsed.id === 'string' && typeof parsed.token === 'string') {
          setSession({
            id: parsed.id,
            token: parsed.token,
            claimCode: parsed.claimCode ?? parsed.claim_code ?? '',
            expiresAt: parsed.expiresAt ?? parsed.expires_at ?? '',
          })
        }
      } catch {
        /* ignore */
      }
    }
    const storedDeliveryAddress = window.localStorage.getItem(STORAGE_KEYS.deliveryAddress)
    if (storedDeliveryAddress) {
      setDeliveryAddress(storedDeliveryAddress)
    }
  }, [])

  useEffect(() => {
    if (typeof window === 'undefined') {
      return
    }
    if (!session?.id) {
      resetTrackingState()
      setSessionHydrationTick(0)
      return
    }

    const hydrateDeposits = () => {
      if (!session?.id) {
        return
      }
      let parsed: StoredDeposit[] = []
      const key = scopedKey(STORAGE_KEYS.deposits, session.id)
      const raw = window.localStorage.getItem(key)
      if (raw) {
        try {
          const json = JSON.parse(raw)
          if (Array.isArray(json)) {
            parsed = json
              .filter((entry): entry is StoredDeposit => typeof entry?.id === 'string')
              .map((entry) => ({ id: entry.id, pickupToken: entry.pickupToken ?? null }))
          }
        } catch {
          /* ignore */
        }
      }
      if (!parsed.length) {
        const legacyKey = window.localStorage.getItem(STORAGE_KEYS.deposits)
        if (legacyKey) {
          try {
            const legacyParsed = JSON.parse(legacyKey)
            if (Array.isArray(legacyParsed)) {
              parsed = legacyParsed
                .filter((entry): entry is StoredDeposit => typeof entry?.id === 'string')
                .map((entry) => ({ id: entry.id, pickupToken: entry.pickupToken ?? null }))
            }
          } catch {
            /* ignore */
          }
          if (parsed.length) {
            persistDeposits(parsed, session.id)
            window.localStorage.removeItem(STORAGE_KEYS.deposits)
          }
        }
      }
      if (!parsed.length) {
        const legacy = window.localStorage.getItem(STORAGE_KEYS.legacyDeposit)
        if (legacy) {
          try {
            const legacyParsed = JSON.parse(legacy)
            if (legacyParsed && typeof legacyParsed === 'object' && typeof legacyParsed.id === 'string') {
              parsed = [{ id: legacyParsed.id, pickupToken: legacyParsed.pickupToken ?? null }]
            } else {
              parsed = [{ id: legacy }]
            }
          } catch {
            parsed = [{ id: legacy }]
          }
          window.localStorage.removeItem(STORAGE_KEYS.legacyDeposit)
        }
      }
      if (parsed.length) {
        persistDeposits(parsed, session.id)
      }
      setDeposits(parsed)
      setSelectedDepositId((current) => {
        if (current && parsed.some((entry) => entry.id === current)) {
          return current
        }
        return parsed[0]?.id ?? null
      })
    }

    const hydrateWithdrawals = () => {
      if (!session?.id) {
        return
      }
      let parsed: StoredWithdrawal[] = []
      const key = scopedKey(STORAGE_KEYS.withdrawals, session.id)
      const raw = window.localStorage.getItem(key)
      if (raw) {
        try {
          const json = JSON.parse(raw)
          if (Array.isArray(json)) {
            parsed = json.filter((entry): entry is StoredWithdrawal => typeof entry?.id === 'string')
          }
        } catch {
          /* ignore */
        }
      }
      if (!parsed.length) {
        const legacyKey = window.localStorage.getItem(STORAGE_KEYS.withdrawals)
        if (legacyKey) {
          try {
            const legacyParsed = JSON.parse(legacyKey)
            if (Array.isArray(legacyParsed)) {
              parsed = legacyParsed.filter((entry): entry is StoredWithdrawal => typeof entry?.id === 'string')
            }
          } catch {
            /* ignore */
          }
          if (parsed.length) {
            persistWithdrawals(parsed, session.id)
            window.localStorage.removeItem(STORAGE_KEYS.withdrawals)
          }
        }
      }
      if (!parsed.length) {
        const legacy = window.localStorage.getItem(STORAGE_KEYS.legacyWithdrawal)
        if (legacy) {
          parsed = [{ id: legacy }]
          window.localStorage.removeItem(STORAGE_KEYS.legacyWithdrawal)
        }
      }
      if (parsed.length) {
        persistWithdrawals(parsed, session.id)
      }
      setWithdrawals(parsed)
      setSelectedWithdrawalId((current) => {
        if (current && parsed.some((entry) => entry.id === current)) {
          return current
        }
        return parsed[0]?.id ?? null
      })
    }

    const hydrateArchived = () => {
      if (!session?.id) {
        return
      }
      const key = scopedKey(STORAGE_KEYS.archives, session.id)
      const raw = window.localStorage.getItem(key)
      if (raw) {
        try {
          const json = JSON.parse(raw)
          if (Array.isArray(json)) {
            const parsed = json.filter((entry): entry is ArchivedEntry =>
              entry && typeof entry.id === 'string' && typeof entry.amount === 'number' && typeof entry.kind === 'string'
            )
            setArchivedEntries(parsed)
            return
          }
        } catch {
          /* ignore */
        }
      }
      setArchivedEntries([])
    }

    hydrateDeposits()
    hydrateWithdrawals()
    hydrateArchived()
    setSessionHydrationTick((prev) => prev + 1)

  }, [session])

  useEffect(() => {
    if (flow !== 'withdrawal') {
      setTokenMintInfo(null)
    }
  }, [flow])

  useEffect(() => {
    if (!resumeFlowHint || sessionHydrationTick === 0) {
      return
    }
    if (deposits.length === 0 && withdrawals.length > 0) {
      setFlow('withdrawal')
    }
    setResumeFlowHint(false)
  }, [resumeFlowHint, sessionHydrationTick, deposits.length, withdrawals.length])

  const rememberDeposit = (id: string, pickupToken: string) => {
    if (!session?.id) {
      return
    }
    setDeposits((current) => {
      const next = [{ id, pickupToken }, ...current.filter((entry) => entry.id !== id)]
      persistDeposits(next, session.id)
      return next
    })
    setSelectedDepositId(id)
  }

  const rememberWithdrawalId = (id: string) => {
    if (!session?.id) {
      return
    }
    setWithdrawals((current) => {
      const next = [{ id }, ...current.filter((entry) => entry.id !== id)]
      persistWithdrawals(next, session.id)
      return next
    })
    setSelectedWithdrawalId(id)
  }

  const addArchivedEntry = (entry: ArchivedEntry) => {
    if (!session?.id) {
      return
    }
    setArchivedEntries((current) => {
      const next = [entry, ...current].slice(0, MAX_ARCHIVED_ENTRIES)
      persistArchived(next, session.id)
      return next
    })
  }

  const archiveDeposit = (deposit: Deposit) => {
    if (!session?.id) {
      return
    }
    if (selectedDepositId === deposit.id) {
      pickupMutation.reset()
    }
    addArchivedEntry({
      id: deposit.id,
      amount: deposit.amount_sats,
      kind: 'deposit',
      archivedAt: new Date().toISOString(),
    })
    setDeposits((current) => {
      const next = current.filter((entry) => entry.id !== deposit.id)
      persistDeposits(next, session.id)
      setSelectedDepositId((currentSelected) => {
        if (currentSelected === deposit.id) {
          return next[0]?.id ?? null
        }
        return currentSelected
      })
      return next
    })
  }

  const archiveWithdrawal = (withdrawal?: Withdrawal) => {
    if (!selectedWithdrawalId || !session?.id) {
      return
    }
    if (withdrawal) {
      addArchivedEntry({
        id: withdrawal.id,
        amount:
          withdrawal.token_value_sats ?? withdrawal.requested_amount_sats ?? 0,
        kind: 'withdrawal',
        archivedAt: new Date().toISOString(),
      })
    }
    const targetId = withdrawal?.id ?? selectedWithdrawalId
    setWithdrawals((current) => {
      const next = current.filter((entry) => entry.id !== targetId)
      persistWithdrawals(next, session.id)
      setSelectedWithdrawalId(next[0]?.id ?? null)
      return next
    })
  }

  const handleDepositPickup = () => {
    if (!selectedDepositId) {
      return
    }
    if (!sessionToken) {
      setMessage('Start or resume a session before revealing tokens.')
      return
    }
    const selected = deposits.find((entry) => entry.id === selectedDepositId)
    const pickupToken = selected?.pickupToken ?? latestDeposit?.pickup_token ?? null
    if (!pickupToken) {
      return
    }
    pickupMutation.mutate({ id: selectedDepositId, pickupToken })
  }

  const handleDeliveryAddressChange = (value: string) => {
    setDeliveryAddress(value)
    if (typeof window === 'undefined') {
      return
    }
    if (value) {
      window.localStorage.setItem(STORAGE_KEYS.deliveryAddress, value)
    } else {
      window.localStorage.removeItem(STORAGE_KEYS.deliveryAddress)
    }
  }

  const {
    data: latestDeposit,
    error: depositError,
    isFetching: depositLoading,
  } = useQuery({
    queryKey: ['deposit', selectedDepositId, sessionToken],
    queryFn: () => getDeposit(selectedDepositId as string, sessionToken ?? undefined),
    enabled: Boolean(selectedDepositId && sessionToken),
    refetchInterval: STATUS_REFRESH_MS,
  })

  useEffect(() => {
    if (!session?.id || !latestDeposit?.id || !latestDeposit.pickup_token) {
      return
    }

    setDeposits((current) => {
      let changed = false
      const next = current.map((entry) => {
        if (entry.id !== latestDeposit.id || entry.pickupToken) {
          return entry
        }
        changed = true
        return { ...entry, pickupToken: latestDeposit.pickup_token ?? null }
      })

      if (changed) {
        persistDeposits(next, session.id)
      }
      return changed ? next : current
    })
  }, [latestDeposit?.id, latestDeposit?.pickup_token, session?.id])

  const {
    data: latestWithdrawal,
    error: withdrawalError,
    isFetching: withdrawalLoading,
  } = useQuery({
    queryKey: ['withdrawal', selectedWithdrawalId, sessionToken],
    queryFn: () => getWithdrawal(selectedWithdrawalId as string, sessionToken ?? undefined),
    enabled: Boolean(selectedWithdrawalId && sessionToken),
    refetchInterval: STATUS_REFRESH_MS,
  })

  useEffect(() => {
    if (depositError) {
      maybeHandleSessionAuthError(depositError)
    }
    if (withdrawalError) {
      maybeHandleSessionAuthError(withdrawalError)
    }
  }, [depositError, withdrawalError])

  const pickupMutation = useMutation({
    mutationFn: ({ id, pickupToken }: { id: string; pickupToken: string }) =>
      pickupDeposit(id, pickupToken, sessionToken ?? undefined),
    onSuccess: async (resp, variables) => {
      const warning = 'Paste it immediately — this app will not display the token again.'
      if (resp?.token) {
        setRevealedTokens((current) => ({ ...current, [variables.id]: resp.token }))
        const copied = await copyTextWithFallback(resp.token)
        if (copied) {
          const notice = `Token revealed and copied to clipboard. ${warning}`
          setMessage(notice)
          showFloatingNotice(notice)
        } else {
          const notice = `Token revealed but automatic copy failed. Token: ${resp.token} ${warning}`
          setMessage(notice)
          showFloatingNotice(notice)
        }
      } else {
        const notice = `Token revealed. ${warning}`
        setMessage(notice)
        showFloatingNotice(notice)
      }
      if (variables?.id) {
        queryClient.invalidateQueries({ queryKey: ['deposit', variables.id] })
        setDeposits((current) => {
          const updated = current.map((entry) =>
            entry.id === variables.id ? { ...entry, pickupToken: null } : entry
          )
          persistDeposits(updated, session?.id)
          return updated
        })
      }
    },
    onError: (err: unknown) => {
      if (maybeHandleSessionAuthError(err)) {
        return
      }
      const normalized = normalizeError(err)
      if (normalized) {
        setMessage(normalized.message)
      }
      if (shouldEscalateToSupport(err, 'pickup')) {
        routeToSupport('pickup_error')
      }
    },
  })

  const handleStartSession = async () => {
    setSessionBusy(true)
    setMessage(null)
    try {
      const response = await startSession()
      applySessionPayload(response)
      setResumeCode('')
      setMessage('New work session started. Write down your claim code to resume later.')
    } catch (err) {
      if (err instanceof ApiClientError) {
        setMessage(`API error (${err.status}): ${err.message}`)
      } else {
        setMessage(`Session error: ${(err as Error).message}`)
      }
    } finally {
      setSessionBusy(false)
    }
  }

  const handleResumeSession = async (evt: FormEvent<HTMLFormElement>) => {
    evt.preventDefault()
    const trimmed = resumeCode.trim()
    if (!trimmed) {
      setMessage('Enter your claim code to resume a session.')
      return
    }
    setSessionBusy(true)
    setMessage(null)
    try {
      const response = await resumeSession(trimmed)
      applySessionPayload(response, { resumed: true })
      setResumeCode('')
      setMessage('Session resumed. Header will refresh automatically.')
    } catch (err) {
      if (!maybeHandleSessionAuthError(err)) {
        if (err instanceof ApiClientError) {
          setMessage(`API error (${err.status}): ${err.message}`)
        } else {
          setMessage(`Session error: ${(err as Error).message}`)
        }
      }
    } finally {
      setSessionBusy(false)
    }
  }

  const handleEndSession = () => {
    dropSession('Session cleared. Start a new one when you’re ready.')
  }

  const handleCopyClaimCode = async () => {
    if (!session?.claimCode) {
      return
    }
    const copied = await copyTextWithFallback(session.claimCode)
    if (copied) {
      setMessage('Claim code copied to clipboard')
    } else {
      setMessage('Unable to copy automatically; please copy the claim code manually.')
    }
  }

  const canonicalMintUrl = limits.cashuMintUrl || (config.cashuMintUrl ? config.cashuMintUrl.trim() : '')

  const handleTokenChange = (value: string) => {
    setToken(value)
    const detected = detectTokenMint(value)
    if (!detected) {
      setTokenMintInfo(null)
      return
    }
    if ('error' in detected) {
      setTokenMintInfo({ error: detected.error })
      return
    }
    const expectedMint = canonicalMintUrl
    const isForeign = Boolean(expectedMint && detected.mintUrl !== expectedMint)
    setTokenMintInfo({ mintUrl: detected.mintUrl, isForeign, amount: detected.amount })
  }

  const handleSubmit = async (evt: FormEvent<HTMLFormElement>) => {
    evt.preventDefault()
    setSubmitting(true)
    setMessage(null)

    try {
      if (!sessionToken) {
        throw new Error('Start or resume a session before submitting a request.')
      }
      if (flow === 'deposit') {
        if (depositFlowDisabled) {
          throw new Error(depositDisabledMessage)
        }
        const requestedAmount = Number(depositAmount)
        if (
          !Number.isFinite(requestedAmount) ||
          requestedAmount < limits.depositMinSats ||
          requestedAmount > limits.depositMaxSats
        ) {
          throw new Error(
            `Deposit amount must be between ${limits.depositMinSats.toLocaleString()} and ${limits.depositMaxSats.toLocaleString()} sats`
          )
        }
        const payload = {
          amount_sats: requestedAmount,
          metadata: { source: 'ui-proto' },
        }
        const creation = await createDeposit(payload, sessionToken)
        rememberDeposit(creation.deposit.id, creation.pickup_token)
        setMessage(
          `Deposit ${creation.deposit.id} → ${creation.deposit.address} (${creation.deposit.state})`
        )
      } else {
        let resolvedAmount = Number(withdrawalAmount)
        const normalizedAddress = deliveryAddress.trim()
        if (!normalizedAddress) {
          throw new Error('Enter a Bitcoin address before submitting the withdrawal.')
        }
        if (!isValidBitcoinAddress(normalizedAddress)) {
          throw new Error('Enter a valid Bitcoin address (bc1…, 1…, or 3…).')
        }
        const payload: CreateWithdrawalRequest = {
          amount_sats: resolvedAmount,
          delivery_address: normalizedAddress,
        }

        if (withdrawalMethod === 'token') {
          const trimmed = token.trim()
          if (!trimmed) {
            throw new Error('Provide a Cashu token or switch to payment requests')
          }

          const decodedAmount = tokenMintInfo && !('error' in tokenMintInfo) ? tokenMintInfo.amount : null
          if (decodedAmount !== null) {
            resolvedAmount = decodedAmount
            if (resolvedAmount < withdrawalMinimum) {
              throw new Error(
                `Token value must be at least ${withdrawalMinimum.toLocaleString()} sats`
              )
            }
          } else {
            resolvedAmount = withdrawalMinimum
          }

          payload.amount_sats = resolvedAmount
          payload.token = trimmed
        } else {
          if (resolvedAmount <= 0 || Number.isNaN(resolvedAmount)) {
            throw new Error('Withdrawal amount must be greater than zero')
          }
        if (resolvedAmount < withdrawalMinimum) {
          throw new Error(
            `Withdrawal amount must be at least ${withdrawalMinimum.toLocaleString()} sats`
          )
        }
          payload.amount_sats = resolvedAmount
          payload.create_payment_request = true
        }

        const withdrawal = await createWithdrawal(payload, sessionToken)
        rememberWithdrawalId(withdrawal.id)
        setMessage(
          `Withdrawal ${withdrawal.id} → ${withdrawal.delivery_address} (${withdrawal.state})`
        )
      }
    } catch (err) {
      if (maybeHandleSessionAuthError(err)) {
        return
      }
      if (err instanceof ApiClientError) {
        setMessage(`API error (${err.status}): ${err.message}`)
      } else {
        setMessage(`Request error: ${(err as Error).message}`)
      }
      if (shouldEscalateToSupport(err, 'request')) {
        routeToSupport('request_error')
      }
    } finally {
      setSubmitting(false)
    }
  }

  const withdrawalMinimum = limits.withdrawalMinSats
  const decodedTokenAmount =
    tokenMintInfo && !('error' in tokenMintInfo) ? tokenMintInfo.amount : null
  const tokenBelowMinimum = Boolean(
    decodedTokenAmount !== null && decodedTokenAmount < withdrawalMinimum
  )
  const hasTokenDetectionError = Boolean(tokenMintInfo && 'error' in tokenMintInfo)
  const depositFlowDisabled = !limits.depositFlowEnabled
  const maintenanceMode =
    limits.operationMode === 'drain' || limits.operationMode === 'halt'
  const depositDisabledMessage =
    limits.depositFlowReason ?? 'Deposits are temporarily disabled. Please contact the operator.'
  const maintenanceMessage =
    limits.operationMode === 'drain'
      ? 'Shuestand is in maintenance mode (drain). New requests are temporarily disabled.'
      : 'Shuestand is in maintenance mode (halt). Processing is paused.'

  const pickupError = pickupMutation.isError
    ? normalizeError(pickupMutation.error)
    : null

  const selectedDeposit = selectedDepositId
    ? deposits.find((entry) => entry.id === selectedDepositId) ?? null
    : null
  const effectivePickupToken =
    selectedDeposit?.pickupToken ?? latestDeposit?.pickup_token ?? null
  const supportVisible = deposits.length > 0 || withdrawals.length > 0

  const hasSession = Boolean(session)
  const headerTitle = hasSession ? 'Configure Your Swaps' : 'Shuestand: Onchain/Cashu Swaps'
  const headerDescription = hasSession
    ? 'Simple kiosk-ready interface for funding Cashu wallets from on-chain bitcoin and redeeming ecash back to addresses.'
    : 'Sessions keep each kiosk run scoped. Start or resume to track deposits and withdrawals under one claim code.'
  const sessionExpiryText = session?.expiresAt ? new Date(session.expiresAt).toLocaleString() : 'soon'
  const sessionSummaryCard = session ? (
    <div className="session-summary-card">
      <div>
        <p className="eyebrow">Active session</p>
        <p className="claim-code">
          Claim code: <code>{session.claimCode}</code>
        </p>
        <p className="helper">Expires {sessionExpiryText}</p>
        <p className="helper subtle">
          Pickup tokens stay cached on this browser until you end the session, then they are wiped automatically.
        </p>
      </div>
      <div className="session-actions">
        <button type="button" onClick={handleCopyClaimCode}>
          Copy code
        </button>
        <button type="button" onClick={handleEndSession} disabled={sessionBusy}>
          End session
        </button>
      </div>
      <div className="session-controls">
        <div className="mode-toggle">
          <button
            className={flow === 'deposit' ? 'active' : ''}
            onClick={() => {
              if (!depositFlowDisabled) {
                setFlow('deposit')
              }
            }}
            type="button"
            disabled={depositFlowDisabled}
            aria-disabled={depositFlowDisabled}
          >
            Bitcoin → Cashu
          </button>
          <button
            className={flow === 'withdrawal' ? 'active' : ''}
            onClick={() => setFlow('withdrawal')}
            type="button"
          >
            Cashu → Bitcoin
          </button>
        </div>
        {depositFlowDisabled && (
          <p className="helper warning">⚠️ {depositDisabledMessage}</p>
        )}
        <p className="helper subtle backend-endpoint">
          Backend: <code>{config.apiBase}</code>
        </p>
      </div>
    </div>
  ) : null
  const panelClassName = hasSession ? 'panel' : 'panel session-only'

  return (
    <main className="app-shell">
      {floatingNotice && (
        <div className="floating-toast">
          <p>{floatingNotice}</p>
        </div>
      )}
      <header>
        <div>
          <p className="eyebrow">shuestand · cashu ↔ bitcoin</p>
          <h1>{headerTitle}</h1>
          <p className="lede">{headerDescription}</p>
        </div>
        <div className="header-actions">
          <div className="theme-toggle" role="group" aria-label="Color theme">
            <button
              type="button"
              className={theme === 'light' ? 'active' : ''}
              onClick={() => onThemeSelect('light')}
            >
              Day
            </button>
            <button
              type="button"
              className={theme === 'dark' ? 'active' : ''}
              onClick={() => onThemeSelect('dark')}
            >
              Night
            </button>
          </div>
          <button
            type="button"
            className="link-button"
            onClick={() => navigate('/operator')}
          >
            Operator console
          </button>
          {supportVisible && (
            <button
              type="button"
              className="link-button"
              onClick={() => routeToSupport('manual_support')}
            >
              Support
            </button>
          )}
        </div>
      </header>

      <section className={panelClassName}>
        {maintenanceMode ? (
          <div className="session-card hero">
            <p className="eyebrow">Maintenance mode</p>
            <h2>Shuestand is temporarily unavailable</h2>
            <p className="helper lead">{maintenanceMessage}</p>
            <p className="helper subtle">Please try again later or contact the operator.</p>
          </div>
        ) : !hasSession ? (
            <div className="session-card hero">
              <p className="eyebrow">Work session</p>
            <h2>Start a work session</h2>
            <p className="helper lead">
              A session groups one or more swaps together. Start a new session anytime (even multiple times per day). If you already have a claim code, use resume below.
            </p>
            <div className="session-actions-large">
              <button
                type="button"
                onClick={handleStartSession}
                disabled={sessionBusy}
              >
                {sessionBusy ? 'Starting…' : 'Start new session'}
              </button>
              <form className="resume-form" onSubmit={handleResumeSession}>
                <div className="resume-row">
                  <label>
                    Claim code (resume existing session)
                    <input
                      type="text"
                      value={resumeCode}
                      onChange={(e) => setResumeCode(e.target.value)}
                      placeholder="ABCD-EFGH-IJKL-MNOP"
                    />
                  </label>
                  <button
                    type="submit"
                    disabled={sessionBusy || !resumeCode.trim()}
                  >
                    Resume session
                  </button>
                </div>
              </form>
            </div>
            <p className="helper subtle">Claim codes expire automatically, copy them somewhere safe.</p>
            {message && <p className="message">{message}</p>}
          </div>
        ) : (
          <>
            {sessionSummaryCard}
            <div className="workspace-column">
              <form onSubmit={handleSubmit}>
              {flow === 'deposit' ? (
                depositFlowDisabled ? (
                  <div className="notice warning">
                    <p className="helper lead">{depositDisabledMessage}</p>
                  </div>
                ) : (
                  <>
                    {canonicalMintUrl && (
                      <p className="helper subtle">
                        Bitcoin → Cashu swaps return ecash of {canonicalMintUrl}.
                      </p>
                    )}
                    <label>
                      Amount (sats): {Number(depositAmount || limits.depositMinSats).toLocaleString()}
                      <input
                        type="range"
                        min={limits.depositMinSats}
                        max={displayedDepositMaxSats}
                        step={DEPOSIT_SLIDER_STEP_SATS}
                        value={Number(depositAmount || limits.depositMinSats)}
                        onChange={(e) => setDepositAmount(e.target.value)}
                        required
                      />
                      <span className="helper">
                        Range {limits.depositMinSats.toLocaleString()}–{displayedDepositMaxSats.toLocaleString()} sats
                      </span>
                    </label>
                  </>
                )
              ) : (
                <>
                  <div className="method-toggle">
                    <span>Submission method</span>
                    <div className="method-options">
                      <label>
                        <input
                          type="radio"
                          name="withdrawal-method"
                          value="token"
                          checked={withdrawalMethod === 'token'}
                          onChange={() => setWithdrawalMethod('token')}
                        />
                        Paste token
                      </label>
                      <label>
                        <input
                          type="radio"
                          name="withdrawal-method"
                          value="payment_request"
                          checked={withdrawalMethod === 'payment_request'}
                          onChange={() => setWithdrawalMethod('payment_request')}
                        />
                        Cashu payment request
                      </label>
                    </div>
                  </div>
                  {withdrawalMethod === 'payment_request' && (
                    <label>
                      Amount (sats)
                      <input
                        type="number"
                        min={withdrawalMinimum}
                        value={withdrawalAmount}
                        onChange={(e) => setWithdrawalAmount(e.target.value)}
                        required={withdrawalMethod === 'payment_request'}
                      />
                      <span className="helper">
                        Minimum {withdrawalMinimum.toLocaleString()} sats
                      </span>
                    </label>
                  )}
                  {withdrawalMethod === 'token' ? (
                    <label>
                      Cashu token
                      <textarea
                        value={token}
                        onChange={(e) => handleTokenChange(e.target.value)}
                        rows={5}
                        placeholder="Paste ecash token here"
                        required={withdrawalMethod === 'token'}
                      />
                      {hasTokenDetectionError && tokenMintInfo && 'error' in tokenMintInfo && (
                        <span className="helper warning">{tokenMintInfo.error}</span>
                      )}
                      {decodedTokenAmount !== null && tokenMintInfo && !('error' in tokenMintInfo) && (
                        <span
                          className={`helper ${
                            tokenMintInfo.isForeign || tokenBelowMinimum ? 'warning' : 'success'
                          }`}
                        >
                          {tokenBelowMinimum
                            ? `Token value is ${decodedTokenAmount.toLocaleString()} sats, but withdrawals require at least ${withdrawalMinimum.toLocaleString()} sats.`
                            : tokenMintInfo.isForeign
                              ? `Foreign token detected (${tokenMintInfo.mintUrl}); will be swapped to the Shuestand mint first. Value: ${decodedTokenAmount.toLocaleString()} sats.`
                              : `Mint detected: ${tokenMintInfo.mintUrl}. Value: ${decodedTokenAmount.toLocaleString()} sats.`}
                        </span>
                      )}
                    </label>
                  ) : (
                    <div className="helper">
                      We'll create a NUT-18 Cashu payment request so you can scan a QR code
                      instead of pasting a token.
                    </div>
                  )}
                  <label>
                    Bitcoin address
                    <input
                      type="text"
                      value={deliveryAddress}
                      onChange={(e) => handleDeliveryAddressChange(e.target.value)}
                      placeholder="bc1q..."
                      required
                    />
                  </label>
                </>
              )}

              <button
                type="submit"
                disabled={isSubmitting || (flow === 'deposit' && depositFlowDisabled)}
              >
                {isSubmitting ? 'Submitting…' : 'Submit request'}
              </button>
            </form>
            </div>

            <aside>
              <h2>Status</h2>
              <p className={message ? 'message' : 'message muted'}>
                {message ?? 'Awaiting action'}
              </p>

              {flow === 'deposit' ? (
                <>
                  {deposits.length > 0 && (
                    <label className="status-select">
                      Tracked deposits
                      <select
                        value={selectedDepositId ?? ''}
                        onChange={(e) => setSelectedDepositId(e.target.value || null)}
                      >
                        {deposits.map((entry) => (
                          <option key={entry.id} value={entry.id}>
                            {entry.id}
                          </option>
                        ))}
                      </select>
                    </label>
                  )}
                  <DepositStatusCard
                    deposit={latestDeposit}
                    error={normalizeError(depositError)}
                    isLoading={depositLoading}
                    hasSubmission={Boolean(selectedDepositId)}
                    pendingDepositTtlSecs={limits.pendingDepositTtlSecs}
                    pickupToken={effectivePickupToken}
                    revealedToken={selectedDepositId ? revealedTokens[selectedDepositId] ?? null : null}
                    onPickup={handleDepositPickup}
                    pickupPending={pickupMutation.isPending}
                    pickupError={pickupError}
                    onClear={deposits.length ? archiveDeposit : undefined}
                  />
                </>
              ) : (
                <>
                  {withdrawals.length > 0 && (
                    <label className="status-select">
                      Tracked withdrawals
                      <select
                        value={selectedWithdrawalId ?? ''}
                        onChange={(e) => setSelectedWithdrawalId(e.target.value || null)}
                      >
                        {withdrawals.map((entry) => (
                          <option key={entry.id} value={entry.id}>
                            {entry.id}
                          </option>
                        ))}
                      </select>
                    </label>
                  )}
                  <WithdrawalStatusCard
                    withdrawal={latestWithdrawal}
                    error={normalizeError(withdrawalError)}
                    isLoading={withdrawalLoading}
                    hasSubmission={Boolean(selectedWithdrawalId)}
                  />
                  {selectedWithdrawalId && latestWithdrawal && (
                    <button
                      type="button"
                      className="link-button"
                      onClick={() => archiveWithdrawal(latestWithdrawal)}
                    >
                      Archive this withdrawal
                    </button>
                  )}
                </>
              )}

              {archivedEntries.length > 0 && (
                <div className="status-block nested">
                  <h4>Archived transactions</h4>
                  <ul className="archived-list">
                    {archivedEntries.map((entry) => (
                      <li key={`${entry.kind}-${entry.id}-${entry.archivedAt}`}>
                        <span className="status-meta code">{entry.id}</span>
                        <span className="status-meta">
                          {entry.kind === 'deposit' ? 'Deposit' : 'Withdrawal'} ·{' '}
                          {formatSats(entry.amount)} sats ·{' '}
                          {new Date(entry.archivedAt).toLocaleTimeString()}
                        </span>
                      </li>
                    ))}
                  </ul>
                </div>
              )}
            </aside>
          </>
        )}
      </section>
      <AppVersion />
    </main>
  )
}
