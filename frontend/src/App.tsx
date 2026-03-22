import type { FormEvent } from 'react'
import { useEffect, useState } from 'react'
import { useQuery } from '@tanstack/react-query'
import './App.css'
import { config } from './config'
import { detectTokenMint } from './lib/cashu'
import {
  ApiClientError,
  createDeposit,
  createWithdrawal,
  getDeposit,
  getWithdrawal,
} from './lib/api'
import {
  DepositStatusCard,
  WithdrawalStatusCard,
} from './components/KioskStatusCards'
import { OperatorPanel } from './components/OperatorPanel'

type Flow = 'deposit' | 'withdrawal'
type ViewMode = 'kiosk' | 'operator'
type TokenMintInfo =
  | { mintUrl: string; isForeign: boolean }
  | { error: string }

const DEFAULT_AMOUNT = '5000'
const STATUS_REFRESH_MS = 5000

const normalizeError = (err: unknown): Error | null => {
  if (!err) return null
  return err instanceof Error ? err : new Error(String(err))
}

export default function App() {
  const [view, setView] = useState<ViewMode>('kiosk')
  const [flow, setFlow] = useState<Flow>('deposit')
  const [amount, setAmount] = useState(DEFAULT_AMOUNT)
  const [deliveryHint, setDeliveryHint] = useState('cashu://wallet/minibits')
  const [token, setToken] = useState('')
  const [tokenMintInfo, setTokenMintInfo] = useState<TokenMintInfo | null>(null)
  const [deliveryAddress, setDeliveryAddress] = useState(
    'bc1qexampledestination'
  )
  const [isSubmitting, setSubmitting] = useState(false)
  const [message, setMessage] = useState<string | null>(null)
  const [latestDepositId, setLatestDepositId] = useState<string | null>(null)
  const [latestWithdrawalId, setLatestWithdrawalId] = useState<string | null>(
    null
  )

  useEffect(() => {
    if (flow !== 'withdrawal') {
      setTokenMintInfo(null)
    }
  }, [flow])

  const {
    data: latestDeposit,
    error: depositError,
    isFetching: depositLoading,
  } = useQuery({
    queryKey: ['deposit', latestDepositId],
    queryFn: () => getDeposit(latestDepositId as string),
    enabled: Boolean(latestDepositId),
    refetchInterval: STATUS_REFRESH_MS,
  })

  const {
    data: latestWithdrawal,
    error: withdrawalError,
    isFetching: withdrawalLoading,
  } = useQuery({
    queryKey: ['withdrawal', latestWithdrawalId],
    queryFn: () => getWithdrawal(latestWithdrawalId as string),
    enabled: Boolean(latestWithdrawalId),
    refetchInterval: STATUS_REFRESH_MS,
  })

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
    const expectedMint = config.cashuMintUrl
    const isForeign = Boolean(expectedMint && detected.mintUrl !== expectedMint)
    setTokenMintInfo({ mintUrl: detected.mintUrl, isForeign })
  }

  const handleSubmit = async (evt: FormEvent<HTMLFormElement>) => {
    evt.preventDefault()
    setSubmitting(true)
    setMessage(null)

    try {
      if (flow === 'deposit') {
        const payload = {
          amount_sats: Number(amount),
          metadata: { source: 'ui-proto' },
          delivery_hint: deliveryHint.trim() || undefined,
        }
        const deposit = await createDeposit(payload)
        setLatestDepositId(deposit.id)
        setMessage(
          `Deposit ${deposit.id} → ${deposit.address} (${deposit.state})`
        )
      } else {
        const payload = {
          token: token.trim(),
          delivery_address: deliveryAddress.trim(),
        }
        const withdrawal = await createWithdrawal(payload)
        setLatestWithdrawalId(withdrawal.id)
        setMessage(
          `Withdrawal ${withdrawal.id} → ${withdrawal.delivery_address} (${withdrawal.state})`
        )
      }
    } catch (err) {
      if (err instanceof ApiClientError) {
        setMessage(`API error (${err.status}): ${err.message}`)
      } else {
        setMessage(`Request error: ${(err as Error).message}`)
      }
    } finally {
      setSubmitting(false)
    }
  }

  return (
    <main className="app-shell">
      <header>
        <div>
          <p className="eyebrow">shuestand · cashu ↔ bitcoin</p>
          <h1>{view === 'kiosk' ? 'Manage your sats float' : 'Operator console'}</h1>
          <p className="lede">
            {view === 'kiosk'
              ? 'Simple kiosk-ready interface for funding Cashu wallets from on-chain bitcoin and redeeming ecash back to addresses.'
              : 'Inspect hot-wallet liquidity, rescan Electrum, and push manual payouts when needed.'}
          </p>
        </div>
        <div className="header-actions">
          <div className="view-toggle">
            <button
              className={view === 'kiosk' ? 'active' : ''}
              type="button"
              onClick={() => setView('kiosk')}
            >
              Kiosk
            </button>
            <button
              className={view === 'operator' ? 'active' : ''}
              type="button"
              onClick={() => setView('operator')}
            >
              Operator
            </button>
          </div>
          {view === 'kiosk' && (
            <div className="mode-toggle">
              <button
                className={flow === 'deposit' ? 'active' : ''}
                onClick={() => setFlow('deposit')}
                type="button"
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
          )}
        </div>
      </header>

      <section className={`panel ${view === 'operator' ? 'operator-mode' : ''}`}>
        {view === 'kiosk' ? (
          <>
            <form onSubmit={handleSubmit}>
              {flow === 'deposit' ? (
                <>
                  <label>
                    Amount (sats)
                    <input
                      type="number"
                      min={1}
                      value={amount}
                      onChange={(e) => setAmount(e.target.value)}
                      required
                    />
                  </label>
                  <label>
                    Delivery target (optional)
                    <input
                      type="text"
                      value={deliveryHint}
                      onChange={(e) => setDeliveryHint(e.target.value)}
                      placeholder="cashu://wallet or numopay order"
                    />
                    <span className="helper">
                      Provide a wallet URL (cashu://, nut://) or an operator label
                      to auto-push tokens when ready.
                    </span>
                  </label>
                </>
              ) : (
                <>
                  <label>
                    Cashu token
                    <textarea
                      value={token}
                      onChange={(e) => handleTokenChange(e.target.value)}
                      rows={5}
                      placeholder="Paste ecash token here"
                      required
                    />
                    {tokenMintInfo &&
                      ('error' in tokenMintInfo ? (
                        <span className="helper warning">{tokenMintInfo.error}</span>
                      ) : (
                        <span
                          className={`helper ${tokenMintInfo.isForeign ? 'warning' : 'success'}`}
                        >
                          {tokenMintInfo.isForeign
                            ? `Foreign token detected (${tokenMintInfo.mintUrl}); will be swapped to the Shuestand mint first.`
                            : `Mint detected: ${tokenMintInfo.mintUrl}`}
                        </span>
                      ))}
                  </label>
                  <label>
                    Bitcoin address
                    <input
                      type="text"
                      value={deliveryAddress}
                      onChange={(e) => setDeliveryAddress(e.target.value)}
                      placeholder="bc1q..."
                      required
                    />
                  </label>
                </>
              )}

              <button type="submit" disabled={isSubmitting}>
                {isSubmitting ? 'Submitting…' : 'Submit request'}
              </button>
            </form>

            <aside>
              <h2>Status</h2>
              <p>
                Backend: <code>{config.apiBase}</code>
              </p>
              <p className={message ? 'message' : 'message muted'}>
                {message ?? 'Awaiting action'}
              </p>

              {flow === 'deposit' ? (
                <DepositStatusCard
                  deposit={latestDeposit}
                  error={normalizeError(depositError)}
                  isLoading={depositLoading}
                  hasSubmission={Boolean(latestDepositId)}
                />
              ) : (
                <WithdrawalStatusCard
                  withdrawal={latestWithdrawal}
                  error={normalizeError(withdrawalError)}
                  isLoading={withdrawalLoading}
                  hasSubmission={Boolean(latestWithdrawalId)}
                />
              )}
            </aside>
          </>
        ) : (
          <OperatorPanel />
        )}
      </section>
    </main>
  )
}
