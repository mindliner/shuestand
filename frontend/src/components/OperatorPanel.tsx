import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { useMemo, useState } from 'react'
import { QRCodeSVG } from 'qrcode.react'
import {
  createCashuInvoice,
  getCashuInvoice,
  getCashuWalletBalance,
  getWalletBalance,
  getWalletTopup,
  getFloatStatus,
  mintCashuInvoice,
  sendCashuToken,
  sendWalletPayment,
  syncWallet,
} from '../lib/api'
import { CopyButton } from './KioskStatusCards'
import type { FloatStatusResponse } from '../types/api'

const formatTokenSnippet = (token: string) => {
  if (token.length <= 120) {
    return token
  }
  return `${token.slice(0, 90)}…${token.slice(-8)}`
}

const formatSats = (value: number) => value.toLocaleString('en-US')

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
  const [topupAmount, setTopupAmount] = useState('')
  const [invoiceAmount, setInvoiceAmount] = useState('50000')
  const [invoiceBolt12, setInvoiceBolt12] = useState(false)
  const [activeQuoteId, setActiveQuoteId] = useState<string | null>(null)
  const [cashuPayoutAmount, setCashuPayoutAmount] = useState('')
  const [cashuTokenOutput, setCashuTokenOutput] = useState<string | null>(null)

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

  const syncMutation = useMutation({
    mutationFn: () => syncWallet(token),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['wallet-balance', token] })
      setFeedback('Wallet sync requested')
    },
  })

  const sendMutation = useMutation({
    mutationFn: () =>
      sendWalletPayment(token, {
        address: payoutForm.address.trim(),
        amount_sats: Number(payoutForm.amount),
        fee_rate_vb: Number(payoutForm.feeRate),
      }),
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

  const mintMutation = useMutation({
    mutationFn: () => mintCashuInvoice(token, activeQuoteId as string),
    onSuccess: (res) => {
      setFeedback(`Minted ${res.amount_sats} sats into Cashu wallet`)
      queryClient.invalidateQueries({ queryKey: ['cashu-balance', token] })
      queryClient.invalidateQueries({
        queryKey: ['cashu-invoice', token, activeQuoteId],
      })
    },
  })

  const cashuSendMutation = useMutation({
    mutationFn: (amount: number) => sendCashuToken(token, { amount_sats: amount }),
    onSuccess: (res, amount) => {
      setCashuTokenOutput(res.token)
      setCashuPayoutAmount('')
      setFeedback(`Exported ${amount} sats as a token`)
    },
  })

  const floatStatusQuery = useQuery({
    queryKey: ['float-status', token],
    queryFn: () => getFloatStatus(token),
    enabled: Boolean(token),
    refetchInterval: token ? 10000 : false,
  })

  const floatStatus = floatStatusQuery.data
  const thresholds = floatStatus
    ? {
        lower: Math.round(floatStatus.target_sats * floatStatus.min_ratio),
        upper: Math.round(floatStatus.target_sats * floatStatus.max_ratio),
      }
    : undefined

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

  const invoice = invoiceQuery.data
  const canMint = Boolean(
    activeQuoteId && invoice && invoice.state?.toLowerCase() === 'paid',
  )

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
          <div className="operator-section">
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
                  <label>
                    Fee rate (sat/vB)
                    <input
                      type="number"
                      min={1}
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
                  </label>
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
                      <button
                        type="button"
                        onClick={() => mintMutation.mutate()}
                        disabled={!canMint || mintMutation.isPending}
                      >
                        {mintMutation.isPending ? 'Minting…' : 'Mint paid invoice'}
                      </button>
                    </div>
                    {mintMutation.isError && (
                      <p className="status-error">{(mintMutation.error as Error).message}</p>
                    )}
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
