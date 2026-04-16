import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { useEffect, useMemo, useState } from 'react'
import { QRCodeSVG } from 'qrcode.react'
import {
  createCashuInvoice,
  getCashuInvoice,
  getCashuWalletBalance,
  getLedgerSnapshot,
  getWalletBalance,
  getWalletTopup,
  getFloatStatus,
  sendCashuToken,
  sendWalletPayment,
  syncWallet,
  listOperatorWithdrawals,
  operateWithdrawal,
  listOperatorDeposits,
  operateDeposit,
  getOperationMode,
  setOperationMode,
  getTransactionCounter,
  getPublicConfig,
} from '../lib/api'
import { CopyButton } from './KioskStatusCards'
import type { FloatStatusResponse, OperatorWithdrawalActionRequest, OperatorDepositActionRequest, Withdrawal, Deposit, OperationMode, FeeEstimateEntry } from '../types/api'

const formatTokenSnippet = (token: string) => {
  if (token.length <= 120) {
    return token
  }
  return `${token.slice(0, 90)}…${token.slice(-8)}`
}

const formatSats = (value: number) => value.toLocaleString('en-US')

const formatSigned = (value: number) => {
  const prefix = value >= 0 ? '+' : '−'
  return `${prefix}${formatSats(Math.abs(value))}`
}

const netClassName = (value: number) => {
  if (value < 0) return 'status-meta warning'
  if (value === 0) return 'status-meta'
  return 'status-meta success'
}

const renderFloatBadge = (status?: FloatStatusResponse['onchain']) => {
  if (!status) {
    return null
  }
  return <span className={`status-pill ${status.state}`}>{status.state}</span>
}

const renderFloatMessage = (
  status?: FloatStatusResponse['onchain'],
  thresholds?: { lower: number; upper: number },
) => {
  if (!status || !thresholds) {
    return null
  }
  if (status.state === 'low') {
    return (
      <p className="status-warning">
        Float below target ({'<' }{formatSats(thresholds.lower)} sats).
      </p>
    )
  }
  if (status.state === 'high') {
    return (
      <p className="status-warning">
        Float above target ({'>'}{formatSats(thresholds.upper)} sats).
      </p>
    )
  }
  if (status.state === 'unknown') {
    return <p className="status-warning">Float status unavailable.</p>
  }
  return null
}

const TOKEN_STORAGE_KEY = 'shuestand.operatorToken'

type CleanupAction = OperatorWithdrawalActionRequest['action']
type DepositCleanupAction = OperatorDepositActionRequest['action']

type OperatorLogEntry = {
  id: string
  kind: 'deposit' | 'withdrawal'
  state: string
  sessionId?: string | null
  createdAt?: string | null
  timestamp: number
}

export function OperatorPanel() {
  const storedToken =
    typeof window !== 'undefined'
      ? window.localStorage.getItem(TOKEN_STORAGE_KEY) ?? ''
      : ''

  const [tokenInput, setTokenInput] = useState(storedToken)
  const [token, setToken] = useState(storedToken)
  const [feedback, setFeedback] = useState<string | null>(null)

  const [payoutForm, setPayoutForm] = useState({
    address: '',
    amount: '',
    feeRate: '2',
  })
  const [payoutFeeMode, setPayoutFeeMode] = useState<'fast' | 'economy' | 'custom'>('fast')
  const [topupAmount, setTopupAmount] = useState('')
  const [invoiceAmount, setInvoiceAmount] = useState('50000')
  const [invoiceBolt12, setInvoiceBolt12] = useState(false)
  const [activeQuoteId, setActiveQuoteId] = useState<string | null>(null)
  const [cashuPayoutAmount, setCashuPayoutAmount] = useState('')
  const [cashuTokenOutput, setCashuTokenOutput] = useState<string | null>(null)
  const [equalizeHint, setEqualizeHint] = useState<string | null>(null)

  const [cleanupNotes, setCleanupNotes] = useState<Record<string, string>>({})
  const [cleanupTxids, setCleanupTxids] = useState<Record<string, string>>({})

  const [depositNotes, setDepositNotes] = useState<Record<string, string>>({})

  const [logEntries, setLogEntries] = useState<OperatorLogEntry[]>([])
  const [logLive, setLogLive] = useState(true)

  const queryClient = useQueryClient()

  const balanceQuery = useQuery({
    queryKey: ['wallet-balance', token],
    queryFn: () => getWalletBalance(token),
    enabled: Boolean(token),
    refetchInterval: token ? 15000 : false,
  })

  const cashuBalanceQuery = useQuery({
    queryKey: ['cashu-balance', token],
    queryFn: () => getCashuWalletBalance(token),
    enabled: Boolean(token),
    refetchInterval: token ? 15000 : false,
  })

  const topupQuery = useQuery({
    queryKey: ['wallet-topup', token],
    queryFn: () => getWalletTopup(token),
    enabled: Boolean(token),
    refetchInterval: false,
  })

  const modeQuery = useQuery({
    queryKey: ['operation-mode', token],
    queryFn: () => getOperationMode(token),
    enabled: Boolean(token),
    refetchInterval: token ? 15000 : false,
  })

  const modeMutation = useMutation({
    mutationFn: (nextMode: OperationMode) => setOperationMode(token, nextMode),
    onSuccess: (res) => {
      setFeedback(`Operations mode set to ${res.mode}`)
      queryClient.invalidateQueries({ queryKey: ['operation-mode', token] })
    },
  })

  const syncMutation = useMutation({
    mutationFn: () => syncWallet(token),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['wallet-balance', token] })
      setFeedback('Wallet sync requested')
    },
  })

  const sendMutation = useMutation({
    mutationFn: () => {
      const manual = Number(payoutForm.feeRate)
      const resolvedFee =
        payoutFeeMode === 'fast' && feeEstimates
          ? feeEstimates.fast.sats_per_vb
          : payoutFeeMode === 'economy' && feeEstimates
            ? feeEstimates.economy.sats_per_vb
            : manual

      const fee_rate_vb = resolvedFee > 0 ? resolvedFee : 0.1

      return sendWalletPayment(token, {
        address: payoutForm.address.trim(),
        amount_sats: Number(payoutForm.amount),
        fee_rate_vb,
      })
    },
    onSuccess: (res) => {
      setFeedback(`Broadcasted tx ${res.txid}`)
      setPayoutForm((prev) => ({ ...prev, amount: '' }))
      queryClient.invalidateQueries({ queryKey: ['wallet-balance', token] })
    },
  })

  const createInvoiceMutation = useMutation({
    mutationFn: () =>
      createCashuInvoice(token, {
        amount_sats: Number(invoiceAmount),
        bolt12: invoiceBolt12,
      }),
    onSuccess: (res) => {
      setActiveQuoteId(res.quote_id)
      setFeedback(`Created ${res.method} quote ${res.quote_id}`)
    },
  })

  const invoiceQuery = useQuery({
    queryKey: ['cashu-invoice', token, activeQuoteId],
    queryFn: () => getCashuInvoice(token, activeQuoteId as string),
    enabled: Boolean(token && activeQuoteId),
    refetchInterval: (query) => {
      const state = (query.state.data as any)?.state as string | undefined
      if (!state) return 5000
      const lower = state.toLowerCase()
      return lower === 'issued' ? false : 5000
    },
  })

  const transactionCounterQuery = useQuery({
    queryKey: ['transaction-counter', token],
    queryFn: () => getTransactionCounter(token),
    enabled: Boolean(token),
    refetchInterval: token ? 60000 : false,
  })

  const publicConfigQuery = useQuery({
    queryKey: ['public-config'],
    queryFn: getPublicConfig,
    staleTime: 5 * 60 * 1000,
  })

  const feeEstimates = publicConfigQuery.data?.fee_estimates ?? null
  const formatFeeValue = (entry?: FeeEstimateEntry | null, fallback?: string) => {
    if (!entry) {
      return fallback ?? '—'
    }
    return `${entry.sats_per_vb.toFixed(1)} sat/vB`
  }
  const formatUpdatedTime = (entry?: FeeEstimateEntry | null) => {
    if (!entry?.updated_at) {
      return '—'
    }
    return new Date(entry.updated_at).toLocaleTimeString()
  }

  const invoice = invoiceQuery.data

  useEffect(() => {
    if (!invoice || !token) {
      return
    }
    if (invoice.state.toLowerCase() === 'issued') {
      queryClient.invalidateQueries({ queryKey: ['cashu-balance', token] })
    }
  }, [invoice, token, queryClient])

  useEffect(() => {
    if (!feeEstimates && payoutFeeMode !== 'custom') {
      setPayoutFeeMode('custom')
    }
  }, [feeEstimates, payoutFeeMode])

  const cashuSendMutation = useMutation({
    mutationFn: (amount: number) => sendCashuToken(token, { amount_sats: amount }),
    onSuccess: (res, amount) => {
      setCashuTokenOutput(res.token)
      setCashuPayoutAmount('')
      setFeedback(`Exported ${amount} sats as a token`)
    },
  })

  const cleanupMutation = useMutation({
    mutationFn: ({ id, payload }: { id: string; payload: OperatorWithdrawalActionRequest }) =>
      operateWithdrawal(token, id, payload),
    onSuccess: (_, variables) => {
      setFeedback('Updated withdrawal state')
      setCleanupNotes((prev) => {
        if (variables.payload.action !== 'mark_failed') {
          return prev
        }
        const next = { ...prev }
        delete next[variables.id]
        return next
      })
      setCleanupTxids((prev) => {
        if (variables.payload.action !== 'mark_settled') {
          return prev
        }
        const next = { ...prev }
        delete next[variables.id]
        return next
      })
      queryClient.invalidateQueries({ queryKey: ['operator-withdrawals', token] })
      queryClient.invalidateQueries({ queryKey: ['ledger', token] })
    },
    onError: (err: unknown) => {
      setFeedback(err instanceof Error ? err.message : 'Cleanup action failed')
    },
  })

  const depositCleanupMutation = useMutation({
    mutationFn: ({ id, payload }: { id: string; payload: OperatorDepositActionRequest }) =>
      operateDeposit(token, id, payload),
    onSuccess: (_, variables) => {
      if (variables.payload.action === 'mark_failed') {
        setDepositNotes((prev) => {
          const next = { ...prev }
          delete next[variables.id]
          return next
        })
      }
      queryClient.invalidateQueries({ queryKey: ['operator-deposits', token] })
      queryClient.invalidateQueries({ queryKey: ['ledger', token] })
    },
    onError: (err: unknown) => {
      setFeedback(err instanceof Error ? err.message : 'Deposit cleanup failed')
    },
  })

  const floatStatusQuery = useQuery({
    queryKey: ['float-status', token],
    queryFn: () => getFloatStatus(token),
    enabled: Boolean(token),
    refetchInterval: token ? 10000 : false,
  })

  const ledgerQuery = useQuery({
    queryKey: ['ledger', token],
    queryFn: () => getLedgerSnapshot(token),
    enabled: Boolean(token),
    refetchInterval: token ? 15000 : false,
  })

  const cleanupQuery = useQuery({
    queryKey: ['operator-withdrawals', token],
    queryFn: () => listOperatorWithdrawals(token),
    enabled: Boolean(token),
    refetchInterval: token ? 20000 : false,
  })

  const depositCleanupQuery = useQuery({
    queryKey: ['operator-deposits', token],
    queryFn: () => listOperatorDeposits(token),
    enabled: Boolean(token),
    refetchInterval: token ? 20000 : false,
  })

  const floatStatus = floatStatusQuery.data
  const thresholds = floatStatus
    ? {
        lower: Math.round(floatStatus.target_sats * floatStatus.min_ratio),
        upper: Math.round(floatStatus.target_sats * floatStatus.max_ratio),
      }
    : undefined
  const ledgerSnapshot = ledgerQuery.data
  const ledgerCapturedAt = ledgerSnapshot ? new Date(ledgerSnapshot.captured_at) : null

  const floatBars = useMemo(() => {
    if (!floatStatus) {
      return null
    }
    const target = floatStatus.target_sats
    const onchain = floatStatus.onchain.balance_sats
    const cashu = floatStatus.cashu.balance_sats
    const max = Math.max(target, onchain, cashu, 1)
    return {
      target,
      targetPct: (target / max) * 100,
      rows: [
        { key: 'onchain', label: 'Onchain', value: onchain, pct: (onchain / max) * 100 },
        { key: 'cashu', label: 'Cashu', value: cashu, pct: (cashu / max) * 100 },
      ],
    }
  }, [floatStatus])

  const handleEqualize = () => {
    if (!floatStatus) {
      setEqualizeHint('Float status not available yet.')
      return
    }

    const target = floatStatus.target_sats
    const onchainDelta = floatStatus.onchain.balance_sats - target
    const cashuDelta = floatStatus.cashu.balance_sats - target

    if (onchainDelta === 0 && cashuDelta === 0) {
      setEqualizeHint('Already exactly on target for both floats.')
      return
    }

    if (onchainDelta > 0 && cashuDelta < 0) {
      const amount = Math.min(onchainDelta, Math.abs(cashuDelta))
      setEqualizeHint(
        `Suggested move: ${formatSats(amount)} sats from Onchain to Cashu (manual rebalance step).`
      )
      return
    }

    if (cashuDelta > 0 && onchainDelta < 0) {
      const amount = Math.min(cashuDelta, Math.abs(onchainDelta))
      setEqualizeHint(
        `Suggested move: ${formatSats(amount)} sats from Cashu to Onchain (manual rebalance step).`
      )
      return
    }

    setEqualizeHint(
      'Both floats are on the same side of target (both surplus or both deficit). Automatic equalize is not available for this case.'
    )
  }

  const handleSaveToken = () => {
    const trimmed = tokenInput.trim()
    setToken(trimmed)
    setFeedback(null)
    setActiveQuoteId(null)
    setCashuTokenOutput(null)
    if (trimmed) {
      window.localStorage.setItem(TOKEN_STORAGE_KEY, trimmed)
    } else {
      window.localStorage.removeItem(TOKEN_STORAGE_KEY)
    }
  }

  const canSendOnchain =
    Boolean(token) &&
    payoutForm.address.trim().length > 0 &&
    Number(payoutForm.amount) > 0 &&
    Number(payoutForm.feeRate) > 0

  const canSendCashu = Boolean(token) && Number(cashuPayoutAmount) > 0

  const cleanupItems = cleanupQuery.data ?? []

  const depositItems = depositCleanupQuery.data ?? []

  const computedLogEntries = useMemo(() => {
    const withdrawals: OperatorLogEntry[] = cleanupItems.map((wd) => ({
      id: wd.id,
      kind: 'withdrawal',
      state: wd.state,
      sessionId: wd.session_id ?? null,
      createdAt: wd.created_at ?? null,
      timestamp: wd.created_at ? new Date(wd.created_at).getTime() : 0,
    }))
    const deposits: OperatorLogEntry[] = depositItems.map((dep) => ({
      id: dep.id,
      kind: 'deposit',
      state: dep.state,
      sessionId: dep.session_id ?? null,
      createdAt: dep.created_at ?? null,
      timestamp: dep.created_at ? new Date(dep.created_at).getTime() : 0,
    }))
    return [...withdrawals, ...deposits].sort((a, b) => b.timestamp - a.timestamp)
  }, [cleanupItems, depositItems])

  useEffect(() => {
    if (logLive) {
      setLogEntries(computedLogEntries)
    }
  }, [computedLogEntries, logLive])

  const handleManualLogRefresh = () => setLogEntries(computedLogEntries)

  const handleCleanupNoteChange = (id: string, value: string) => {
    setCleanupNotes((prev) => {
      if (!value) {
        if (!prev[id]) return prev
        const next = { ...prev }
        delete next[id]
        return next
      }
      return { ...prev, [id]: value }
    })
  }

  const handleCleanupTxidChange = (id: string, value: string) => {
    setCleanupTxids((prev) => {
      if (!value) {
        if (!prev[id]) return prev
        const next = { ...prev }
        delete next[id]
        return next
      }
      return { ...prev, [id]: value }
    })
  }

  const handleCleanupAction = (withdrawal: Withdrawal, action: CleanupAction) => {
    if (!token) return
    const payload: OperatorWithdrawalActionRequest = { action }
    if (action === 'mark_failed') {
      const note = (cleanupNotes[withdrawal.id] ?? '').trim()
      if (note) {
        payload.note = note
      }
    }
    if (action === 'mark_settled') {
      const entered = (cleanupTxids[withdrawal.id] ?? '').trim()
      const fallback = withdrawal.txid ?? ''
      const txidValue = entered || fallback
      if (txidValue) {
        payload.txid = txidValue
      }
    }
    cleanupMutation.mutate({ id: withdrawal.id, payload })
  }

  const handleDepositNoteChange = (id: string, value: string) => {
    setDepositNotes((prev) => {
      if (!value) {
        if (!prev[id]) return prev
        const next = { ...prev }
        delete next[id]
        return next
      }
      return { ...prev, [id]: value }
    })
  }

  const handleDepositAction = (deposit: Deposit, action: DepositCleanupAction) => {
    if (!token) return
    const payload: OperatorDepositActionRequest = { action }
    if (action === 'mark_failed') {
      const note = (depositNotes[deposit.id] ?? '').trim()
      if (note) {
        payload.note = note
      }
    }
    depositCleanupMutation.mutate({ id: deposit.id, payload })
  }

  const topupBip21 = useMemo(() => {
    const uri = topupQuery.data?.bip21_uri
    if (!uri) return null
    if (!topupAmount.trim()) return uri
    const sats = Number(topupAmount)
    if (!Number.isFinite(sats) || sats <= 0) return uri
    const btc = (sats / 1e8).toFixed(8).replace(/\.0+$/, '')
    const joiner = uri.includes('?') ? '&' : '?'
    return `${uri}${joiner}amount=${encodeURIComponent(btc)}`
  }, [topupQuery.data?.bip21_uri, topupAmount])

  const autoMintHint =
    activeQuoteId && invoice && invoice.state?.toLowerCase() === 'paid'
      ? 'Paid — minting into float automatically…'
      : null

  const modeOptions: { value: OperationMode; label: string }[] = [
    { value: 'normal', label: 'Normal' },
    { value: 'drain', label: 'Drain' },
    { value: 'halt', label: 'Halt' },
  ]

  const modeDescriptions: Record<OperationMode, string> = {
    normal: 'Accept new deposits and withdrawals. Background workers run normally.',
    drain: 'Finish existing deposits/withdrawals but reject new ones until you resume.',
    halt: 'Pause all processing. Existing jobs stay visible but workers are idle.',
  }

  const currentMode: OperationMode = modeQuery.data?.mode ?? 'normal'

  const handleModeChange = (next: OperationMode) => {
    if (!token || currentMode === next) {
      return
    }
    modeMutation.mutate(next)
  }

  return (
    <div className="operator-panel">
      <section className="operator-card">
        <h3>Operator access</h3>
        <label>
          Wallet API token
          <input
            type="password"
            value={tokenInput}
            onChange={(e) => setTokenInput(e.target.value)}
            placeholder="WALLET_API_TOKEN"
          />
        </label>
        <button type="button" onClick={handleSaveToken}>
          {token ? 'Update token' : 'Set token'}
        </button>
        {!token && (
          <p className="status-error">Token required for operator actions.</p>
        )}
      </section>

      {token && (
        <>
          <section className="operator-card">
            <div className="operator-card-header">
              <h3>Operations mode</h3>
              {!modeQuery.isLoading && !modeQuery.isError && (
                <span className={`status-pill ${currentMode}`}>
                  {currentMode}
                </span>
              )}
            </div>
            {modeQuery.isLoading ? (
              <p>Loading…</p>
            ) : modeQuery.isError ? (
              <p className="status-error">{(modeQuery.error as Error).message}</p>
            ) : (
              <>
                <p>{modeDescriptions[currentMode]}</p>
                <div className="button-row segmented">
                  {modeOptions.map(({ value, label }) => (
                    <button
                      key={value}
                      type="button"
                      className={value === currentMode ? 'primary' : 'secondary'}
                      onClick={() => handleModeChange(value)}
                      disabled={modeMutation.isPending || value === currentMode}
                    >
                      {label}
                    </button>
                  ))}
                </div>
                {modeMutation.isPending && (
                  <p className="status-meta">Updating mode…</p>
                )}
                {modeMutation.isError && (
                  <p className="status-error">
                    {(modeMutation.error as Error).message}
                  </p>
                )}
              </>
            )}
          </section>

          <section className="operator-card">
            <div className="operator-card-header">
              <h3>Ledger & reconciliation</h3>
              <button
                type="button"
                onClick={() => ledgerQuery.refetch()}
                disabled={ledgerQuery.isFetching}
              >
                Refresh
              </button>
            </div>
            {ledgerQuery.isLoading ? (
              <p>Loading…</p>
            ) : ledgerQuery.isError ? (
              <p className="status-error">{(ledgerQuery.error as Error).message}</p>
            ) : ledgerSnapshot ? (
              <>
                <p className="status-meta">
                  Snapshot {ledgerCapturedAt?.toLocaleTimeString() ?? '—'} · Assets {formatSats(ledgerSnapshot.totals.assets_sats)} sats · Liabilities {formatSats(ledgerSnapshot.totals.liabilities_sats)} sats ·
                  <span className={netClassName(ledgerSnapshot.totals.net_sats)}>
                    Net {formatSigned(ledgerSnapshot.totals.net_sats)} sats
                  </span>
                </p>
                <div className="ledger-breakdown">
                  <div>
                    <h4>Cashu</h4>
                    <p>Assets: {formatSats(ledgerSnapshot.cashu.assets.available_sats)} sats</p>
                    <p>Liabilities: {formatSats(ledgerSnapshot.cashu.liabilities.total_sats)} sats</p>
                    <p className={netClassName(ledgerSnapshot.cashu.net_sats)}>
                      Net {formatSigned(ledgerSnapshot.cashu.net_sats)} sats
                    </p>
                    <ul>
                      <li>
                        Awaiting confirmations: {formatSats(ledgerSnapshot.cashu.liabilities.awaiting_confirmations.amount_sats)} sats · {ledgerSnapshot.cashu.liabilities.awaiting_confirmations.count} deposits
                      </li>
                      <li>
                        Minting/delivering: {formatSats(ledgerSnapshot.cashu.liabilities.minting.amount_sats)} sats · {ledgerSnapshot.cashu.liabilities.minting.count} deposits
                      </li>
                      <li>
                        Ready for pickup: {formatSats(ledgerSnapshot.cashu.liabilities.ready.amount_sats)} sats · {ledgerSnapshot.cashu.liabilities.ready.count} deposits
                      </li>
                    </ul>
                  </div>
                  <div>
                    <h4>On-chain</h4>
                    <p>
                      Assets: {formatSats(ledgerSnapshot.onchain.assets.available_sats)} sats
                      {' '}
                      <span className="status-meta">
                        (confirmed {formatSats(ledgerSnapshot.onchain.assets.confirmed)} · trusted pending {formatSats(ledgerSnapshot.onchain.assets.trusted_pending)})
                      </span>
                    </p>
                    <p>Liabilities: {formatSats(ledgerSnapshot.onchain.liabilities.total_sats)} sats</p>
                    <p className={netClassName(ledgerSnapshot.onchain.net_sats)}>
                      Net {formatSigned(ledgerSnapshot.onchain.net_sats)} sats
                    </p>
                    <ul>
                      <li>
                        Awaiting tokens: {formatSats(ledgerSnapshot.onchain.liabilities.funding.amount_sats)} sats · {ledgerSnapshot.onchain.liabilities.funding.count} requests
                      </li>
                      <li>
                        Queued: {formatSats(ledgerSnapshot.onchain.liabilities.queued.amount_sats)} sats · {ledgerSnapshot.onchain.liabilities.queued.count} withdrawals
                      </li>
                      <li>
                        Broadcasting: {formatSats(ledgerSnapshot.onchain.liabilities.broadcasting.amount_sats)} sats · {ledgerSnapshot.onchain.liabilities.broadcasting.count} withdrawals
                      </li>
                      <li>
                        Confirming: {formatSats(ledgerSnapshot.onchain.liabilities.confirming.amount_sats)} sats · {ledgerSnapshot.onchain.liabilities.confirming.count} withdrawals
                      </li>
                    </ul>
                  </div>
                </div>
              </>
            ) : (
              <p>No ledger data yet.</p>
            )}
          </section>

          <section className="operator-card">
            <div className="operator-card-header">
              <div className="operator-card-title">
                <h3>Activity log</h3>
              </div>
              <label className="checkbox-row">
                <input
                  type="checkbox"
                  checked={logLive}
                  onChange={(e) => {
                    const next = e.target.checked
                    setLogLive(next)
                    if (next) {
                      setLogEntries(computedLogEntries)
                    }
                  }}
                />
                Live
              </label>
              {!logLive && (
                <button type="button" onClick={handleManualLogRefresh}>
                  Update now
                </button>
              )}
            </div>
            {logEntries.length === 0 ? (
              <p>No deposits or withdrawals are in flight.</p>
            ) : (
              <ul className="operator-log">
                {logEntries.map((entry) => (
                  <li key={`${entry.kind}-${entry.id}`} className="operator-log-entry">
                    <div className="stacked">
                      <span className="status-meta">
                        {entry.createdAt
                          ? new Date(entry.createdAt).toLocaleTimeString()
                          : '—'}
                      </span>
                      <span className="status-meta">
                        Session {entry.sessionId ?? '—'}
                      </span>
                    </div>
                    <div className="log-entry-body">
                      <strong>
                        {entry.kind === 'withdrawal' ? 'Withdrawal' : 'Deposit'} {entry.id}
                      </strong>
                      <span className="status-pill">{entry.state}</span>
                    </div>
                  </li>
                ))}
              </ul>
            )}
            {typeof transactionCounterQuery.data?.count === 'number' && (
              <p className="operator-card-footnote">
                {transactionCounterQuery.data.count.toLocaleString('en-US')} transaction
                {transactionCounterQuery.data.count === 1 ? '' : 's'} completed
              </p>
            )}
          </section>

          <section className="operator-card">
            <div className="operator-card-header">
              <h3>Ongoing withdrawals</h3>
              <button type="button" onClick={() => cleanupQuery.refetch()} disabled={cleanupQuery.isFetching}>
                Refresh
              </button>
            </div>
            {cleanupQuery.isLoading ? (
              <p>Loading…</p>
            ) : cleanupQuery.isError ? (
              <p className="status-error">{(cleanupQuery.error as Error).message}</p>
            ) : cleanupItems.length === 0 ? (
              <p>No withdrawals need manual cleanup right now.</p>
            ) : (
              <div className="operator-table-scroll">
                <table className="operator-table condensed-table">
                  <thead>
                    <tr>
                      <th>ID</th>
                      <th>State</th>
                      <th>Amount</th>
                      <th>Address</th>
                      <th>Txid</th>
                      <th>Actions</th>
                    </tr>
                  </thead>
                  <tbody>
                    {cleanupItems.map((wd) => {
                      const amount = wd.token_value_sats ?? wd.requested_amount_sats ?? 0
                      const createdAt = wd.created_at ? new Date(wd.created_at).toLocaleString() : '—'
                      const canSettle = wd.state !== 'settled' && wd.state !== 'funding'
                      const canFail = wd.state !== 'settled'
                      const canRequeue =
                        wd.state === 'failed' || wd.state === 'broadcasting' || wd.state === 'confirming'
                      const canArchive = wd.state === 'failed' || wd.state === 'funding'
                      return (
                        <tr key={wd.id}>
                          <td>
                            <div className="stacked">
                              <span className="status-meta code">{wd.id}</span>
                              <span className="status-meta">{createdAt}</span>
                            </div>
                          </td>
                          <td>
                            <span className="status-pill compact">{wd.state}</span>
                          </td>
                          <td>{formatSats(amount)} sats</td>
                          <td className="status-meta code">{wd.delivery_address}</td>
                          <td className="status-meta code">{wd.txid ?? '—'}</td>
                          <td className="actions-cell">
                            <div className="table-inputs">
                              <input
                                type="text"
                                aria-label="Failure note"
                                className="table-input"
                                value={cleanupNotes[wd.id] ?? ''}
                                onChange={(e) => handleCleanupNoteChange(wd.id, e.target.value)}
                                placeholder="Failure note"
                              />
                              <input
                                type="text"
                                aria-label="Override txid"
                                className="table-input"
                                value={cleanupTxids[wd.id] ?? ''}
                                onChange={(e) => handleCleanupTxidChange(wd.id, e.target.value)}
                                placeholder="Override txid"
                              />
                            </div>
                            <div className="button-row table-buttons">
                              <button
                                type="button"
                                className="secondary"
                                onClick={() => handleCleanupAction(wd, 'mark_settled')}
                                disabled={!token || !canSettle || cleanupMutation.isPending}
                              >
                                Mark settled
                              </button>
                              <button
                                type="button"
                                className="secondary"
                                onClick={() => handleCleanupAction(wd, 'mark_failed')}
                                disabled={!token || !canFail || cleanupMutation.isPending}
                              >
                                Mark failed
                              </button>
                              <button
                                type="button"
                                className="secondary"
                                onClick={() => handleCleanupAction(wd, 'requeue')}
                                disabled={!token || !canRequeue || cleanupMutation.isPending}
                              >
                                Requeue
                              </button>
                              <button
                                type="button"
                                className="secondary"
                                onClick={() => handleCleanupAction(wd, 'archive')}
                                disabled={!token || !canArchive || cleanupMutation.isPending}
                              >
                                Archive
                              </button>
                            </div>
                          </td>
                        </tr>
                      )
                    })}
                  </tbody>
                </table>
              </div>
            )}
            {cleanupMutation.isError && (
              <p className="status-error">{(cleanupMutation.error as Error).message}</p>
            )}
          </section>

          <section className="operator-card">
            <div className="operator-card-header">
              <h3>Ongoing deposits</h3>
              <button
                type="button"
                onClick={() => depositCleanupQuery.refetch()}
                disabled={depositCleanupQuery.isFetching}
              >
                Refresh
              </button>
            </div>
            {depositCleanupQuery.isLoading ? (
              <p>Loading…</p>
            ) : depositCleanupQuery.isError ? (
              <p className="status-error">{(depositCleanupQuery.error as Error).message}</p>
            ) : depositItems.length === 0 ? (
              <p>No deposits need manual cleanup right now.</p>
            ) : (
              <div className="operator-table-scroll">
                <table className="operator-table condensed-table">
                  <thead>
                    <tr>
                      <th>ID</th>
                      <th>State</th>
                      <th>Amount</th>
                      <th>Address</th>
                      <th>Confs</th>
                      <th>Actions</th>
                    </tr>
                  </thead>
                  <tbody>
                    {depositItems.map((dep) => {
                      const createdAt = dep.created_at ? new Date(dep.created_at).toLocaleString() : '—'
                      const canFulfill = dep.state === 'ready' || dep.state === 'delivering'
                      const canFail = dep.state !== 'fulfilled' && dep.state !== 'archived_by_operator'
                      const canArchive = dep.state === 'failed' || dep.state === 'fulfilled'
                      return (
                        <tr key={dep.id}>
                          <td>
                            <div className="stacked">
                              <span className="status-meta code">{dep.id}</span>
                              <span className="status-meta">{createdAt}</span>
                            </div>
                          </td>
                          <td>
                            <span className="status-pill compact">{dep.state}</span>
                          </td>
                          <td>{formatSats(dep.amount_sats)} sats</td>
                          <td className="status-meta code">{dep.address}</td>
                          <td>
                            <span className="status-meta">
                              {dep.confirmations} / {dep.target_confirmations}
                            </span>
                          </td>
                          <td className="actions-cell">
                            <div className="table-inputs">
                              <input
                                type="text"
                                aria-label="Failure note"
                                className="table-input"
                                value={depositNotes[dep.id] ?? ''}
                                onChange={(e) => handleDepositNoteChange(dep.id, e.target.value)}
                                placeholder="Failure note"
                              />
                            </div>
                            <div className="button-row table-buttons">
                              <button
                                type="button"
                                className="secondary"
                                onClick={() => handleDepositAction(dep, 'mark_fulfilled')}
                                disabled={!token || !canFulfill || depositCleanupMutation.isPending}
                              >
                                Mark fulfilled
                              </button>
                              <button
                                type="button"
                                className="secondary"
                                onClick={() => handleDepositAction(dep, 'mark_failed')}
                                disabled={!token || !canFail || depositCleanupMutation.isPending}
                              >
                                Mark failed
                              </button>
                              <button
                                type="button"
                                className="secondary"
                                onClick={() => handleDepositAction(dep, 'archive')}
                                disabled={!token || !canArchive || depositCleanupMutation.isPending}
                              >
                                Archive
                              </button>
                            </div>
                          </td>
                        </tr>
                      )
                    })}
                  </tbody>
                </table>
              </div>
            )}
            {depositCleanupMutation.isError && (
              <p className="status-error">{(depositCleanupMutation.error as Error).message}</p>
            )}
          </section>

          <div className="operator-section">
            <div className="section-heading">
              <h2>Float overview</h2>
            </div>
            <section className="operator-card float-overview-card">
              {floatStatusQuery.isLoading ? (
                <p>Loading…</p>
              ) : !floatBars ? (
                <p className="status-error">Float status unavailable.</p>
              ) : (
                <>
                  {floatBars.rows.map((row) => (
                    <div key={row.key} className="float-row">
                      <div className="float-row-head">
                        <strong>{row.label}</strong>
                        <span>{formatSats(row.value)} sats</span>
                      </div>
                      <div className="float-bar-track" role="img" aria-label={`${row.label} float bar`}>
                        <div className="float-bar-fill" style={{ width: `${Math.min(100, row.pct)}%` }} />
                        <div
                          className="float-target-marker"
                          style={{ left: `${Math.min(100, Math.max(0, floatBars.targetPct))}%` }}
                          title={`Target ${formatSats(floatBars.target)} sats`}
                        />
                      </div>
                    </div>
                  ))}
                  <p className="status-meta">Target: {formatSats(floatBars.target)} sats</p>
                </>
              )}
              <button type="button" onClick={handleEqualize} disabled={!floatStatus}>
                Equalize
              </button>
              {equalizeHint && <p className="status-meta">{equalizeHint}</p>}
            </section>

            <div className="section-heading">
              <h2>Onchain</h2>
              {renderFloatBadge(floatStatus?.onchain)}
            </div>
            {floatStatusQuery.isError && (
              <p className="status-error">{(floatStatusQuery.error as Error).message}</p>
            )}
            {renderFloatMessage(floatStatus?.onchain, thresholds)}
            <div className="operator-grid">
              <section className="operator-card">
                <div className="operator-card-header">
                  <h3>Balance</h3>
                  <button
                    type="button"
                    onClick={() => balanceQuery.refetch()}
                    disabled={balanceQuery.isFetching}
                  >
                    Refresh
                  </button>
                </div>
                {balanceQuery.isLoading ? (
                  <p>Loading…</p>
                ) : balanceQuery.isError ? (
                  <p className="status-error">{(balanceQuery.error as Error).message}</p>
                ) : balanceQuery.data ? (
                  <ul className="balance-grid">
                    <li>
                      <span>Confirmed</span>
                      <strong>{balanceQuery.data.confirmed} sats</strong>
                    </li>
                    <li>
                      <span>Trusted pending</span>
                      <strong>{balanceQuery.data.trusted_pending} sats</strong>
                    </li>
                    <li>
                      <span>Untrusted pending</span>
                      <strong>{balanceQuery.data.untrusted_pending} sats</strong>
                    </li>
                    <li>
                      <span>Immature</span>
                      <strong>{balanceQuery.data.immature} sats</strong>
                    </li>
                  </ul>
                ) : null}
                <button
                  type="button"
                  className="secondary"
                  onClick={() => syncMutation.mutate()}
                  disabled={syncMutation.isPending}
                >
                  {syncMutation.isPending ? 'Syncing…' : 'Sync wallet now'}
                </button>
                {syncMutation.isError && (
                  <p className="status-error">{(syncMutation.error as Error).message}</p>
                )}
              </section>

              <section className="operator-card">
                <div className="operator-card-header">
                  <h3>Top-up address</h3>
                  <button
                    type="button"
                    onClick={() => topupQuery.refetch()}
                    disabled={topupQuery.isFetching}
                  >
                    New address
                  </button>
                </div>
                {topupQuery.isError && (
                  <p className="status-error">{(topupQuery.error as Error).message}</p>
                )}
                {topupQuery.data && (
                  <>
                    <p className="status-meta code">{topupQuery.data.address}</p>
                    <CopyButton label="Copy address" text={topupQuery.data.address} />
                    <label>
                      Amount for BIP21 (optional)
                      <input
                        type="number"
                        min={1}
                        value={topupAmount}
                        onChange={(e) => setTopupAmount(e.target.value)}
                        placeholder="e.g. 250000"
                      />
                    </label>
                    {topupBip21 && (
                      <div className="qr-card">
                        <QRCodeSVG value={topupBip21} size={132} />
                        <CopyButton label="Copy BIP21" text={topupBip21} />
                      </div>
                    )}
                  </>
                )}
              </section>

              <section className="operator-card">
                <h3>Manual payout</h3>
                <form
                  onSubmit={(evt) => {
                    evt.preventDefault()
                    if (canSendOnchain) {
                      sendMutation.mutate()
                    }
                  }}
                >
                  <label>
                    Destination address
                    <input
                      type="text"
                      value={payoutForm.address}
                      onChange={(e) =>
                        setPayoutForm((prev) => ({
                          ...prev,
                          address: e.target.value,
                        }))
                      }
                      placeholder="bc1q…"
                      required
                    />
                  </label>
                  <label>
                    Amount (sats)
                    <input
                      type="number"
                      min={1}
                      value={payoutForm.amount}
                      onChange={(e) =>
                        setPayoutForm((prev) => ({
                          ...prev,
                          amount: e.target.value,
                        }))
                      }
                      required
                    />
                  </label>
                  <div className="fee-mode-section">
                    <span className="helper">Fee preference</span>
                    <div className="fee-mode-options">
                      <label className={`fee-mode-option ${!feeEstimates ? 'disabled' : ''}`}>
                        <input
                          type="radio"
                          name="onchain-fee-mode"
                          value="fast"
                          disabled={!feeEstimates}
                          checked={Boolean(feeEstimates) && payoutFeeMode === 'fast'}
                          onChange={() => setPayoutFeeMode('fast')}
                        />
                        <div>
                          <strong>Fast (next block)</strong>
                          <span className="fee-mode-rate">{formatFeeValue(feeEstimates?.fast, '…')}</span>
                        </div>
                      </label>
                      <label className={`fee-mode-option ${!feeEstimates ? 'disabled' : ''}`}>
                        <input
                          type="radio"
                          name="onchain-fee-mode"
                          value="economy"
                          disabled={!feeEstimates}
                          checked={Boolean(feeEstimates) && payoutFeeMode === 'economy'}
                          onChange={() => setPayoutFeeMode('economy')}
                        />
                        <div>
                          <strong>Economy (≈3 blocks)</strong>
                          <span className="fee-mode-rate">{formatFeeValue(feeEstimates?.economy, '…')}</span>
                        </div>
                      </label>
                      <label className="fee-mode-option">
                        <input
                          type="radio"
                          name="onchain-fee-mode"
                          value="custom"
                          checked={!feeEstimates || payoutFeeMode === 'custom'}
                          onChange={() => setPayoutFeeMode('custom')}
                        />
                        <div>
                          <strong>Custom</strong>
                          <span className="fee-mode-rate">Enter manually</span>
                        </div>
                      </label>
                    </div>
                    {feeEstimates && (
                      <p className="status-meta">
                        Fast updated {formatUpdatedTime(feeEstimates.fast)} · Economy {formatUpdatedTime(feeEstimates.economy)}
                      </p>
                    )}
                  </div>
                  {payoutFeeMode === 'custom' || !feeEstimates ? (
                    <label>
                      Fee rate (sat/vB)
                      <input
                        type="number"
                        min={0.1}
                        step={0.1}
                        value={payoutForm.feeRate}
                        onChange={(e) =>
                          setPayoutForm((prev) => ({
                            ...prev,
                            feeRate: e.target.value,
                          }))
                        }
                        required
                      />
                      <span className="helper">Enter a custom fee rate</span>
                    </label>
                  ) : (
                    <p className="helper">
                      Using {payoutFeeMode === 'fast' ? 'fast' : 'economy'} estimate
                      {' '}
                      ({formatFeeValue(
                        payoutFeeMode === 'fast' ? feeEstimates?.fast : feeEstimates?.economy,
                        '—',
                      )}).
                    </p>
                  )}
                  <button type="submit" disabled={!canSendOnchain || sendMutation.isPending}>
                    {sendMutation.isPending ? 'Broadcasting…' : 'Send payout'}
                  </button>
                </form>
                {sendMutation.isError && (
                  <p className="status-error">{(sendMutation.error as Error).message}</p>
                )}
              </section>
            </div>
          </div>

          <div className="operator-section">
            <div className="section-heading">
              <h2>Cashu</h2>
              {renderFloatBadge(floatStatus?.cashu)}
            </div>
            {renderFloatMessage(floatStatus?.cashu, thresholds)}
            <div className="operator-grid">
              <section className="operator-card">
                <div className="operator-card-header">
                  <h3>Wallet balance</h3>
                  <button
                    type="button"
                    onClick={() => cashuBalanceQuery.refetch()}
                    disabled={cashuBalanceQuery.isFetching}
                  >
                    Refresh
                  </button>
                </div>
                {cashuBalanceQuery.isLoading ? (
                  <p>Loading…</p>
                ) : cashuBalanceQuery.isError ? (
                  <p className="status-error">{(cashuBalanceQuery.error as Error).message}</p>
                ) : cashuBalanceQuery.data ? (
                  <ul className="balance-grid">
                    <li>
                      <span>Spendable</span>
                      <strong>{cashuBalanceQuery.data.spendable} sats</strong>
                    </li>
                    <li>
                      <span>Pending</span>
                      <strong>{cashuBalanceQuery.data.pending} sats</strong>
                    </li>
                    <li>
                      <span>Reserved</span>
                      <strong>{cashuBalanceQuery.data.reserved} sats</strong>
                    </li>
                  </ul>
                ) : null}
              </section>

              <section className="operator-card">
                <h3>Funding (mint)</h3>
                <form
                  onSubmit={(evt) => {
                    evt.preventDefault()
                    setFeedback(null)
                    createInvoiceMutation.mutate()
                  }}
                >
                  <label>
                    Amount (sats)
                    <input
                      type="number"
                      min={1}
                      value={invoiceAmount}
                      onChange={(e) => setInvoiceAmount(e.target.value)}
                      required
                    />
                  </label>
                  <label className="checkbox-row">
                    <input
                      type="checkbox"
                      checked={invoiceBolt12}
                      onChange={(e) => setInvoiceBolt12(e.target.checked)}
                    />
                    Use Bolt12
                  </label>
                  <button type="submit" disabled={createInvoiceMutation.isPending}>
                    {createInvoiceMutation.isPending ? 'Requesting…' : 'Request invoice'}
                  </button>
                </form>
                {createInvoiceMutation.isError && (
                  <p className="status-error">
                    {(createInvoiceMutation.error as Error).message}
                  </p>
                )}
                {activeQuoteId && (
                  <p className="status-meta code">quote: {activeQuoteId}</p>
                )}
                {invoiceQuery.isError && (
                  <p className="status-error">{(invoiceQuery.error as Error).message}</p>
                )}
                {invoice && (
                  <>
                    <p>
                      State: <strong>{invoice.state}</strong> ({invoice.method})
                    </p>
                    <div className="qr-card">
                      <QRCodeSVG value={invoice.request} size={132} />
                      <CopyButton label="Copy invoice" text={invoice.request} />
                    </div>
                    <div className="button-row">
                      <button
                        type="button"
                        className="secondary"
                        onClick={() => invoiceQuery.refetch()}
                        disabled={invoiceQuery.isFetching}
                      >
                        Refresh status
                      </button>
                    </div>
                    {autoMintHint && <p className="helper">{autoMintHint}</p>}
                  </>
                )}
              </section>

              <section className="operator-card">
                <h3>Manual payout</h3>
                <form
                  onSubmit={(evt) => {
                    evt.preventDefault()
                    if (canSendCashu) {
                      cashuSendMutation.mutate(Number(cashuPayoutAmount))
                    }
                  }}
                >
                  <label>
                    Amount (sats)
                    <input
                      type="number"
                      min={1}
                      value={cashuPayoutAmount}
                      onChange={(e) => setCashuPayoutAmount(e.target.value)}
                      required
                    />
                  </label>
                  <button type="submit" disabled={!canSendCashu || cashuSendMutation.isPending}>
                    {cashuSendMutation.isPending ? 'Exporting…' : 'Export token'}
                  </button>
                </form>
                {cashuSendMutation.isError && (
                  <p className="status-error">{(cashuSendMutation.error as Error).message}</p>
                )}
                {cashuTokenOutput && (
                  <div className="token-card">
                    <p>Token ready</p>
                    <p className="status-meta code">{formatTokenSnippet(cashuTokenOutput)}</p>
                    <CopyButton label="Copy token" text={cashuTokenOutput} />
                  </div>
                )}
              </section>
            </div>
          </div>
        </>
      )}

      {feedback && <p className="operator-feedback">{feedback}</p>}
    </div>
  )
}
