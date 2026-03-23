import type { FormEvent } from 'react'
import { useEffect, useState } from 'react'

const STORAGE_KEYS = {
  deposit: 'shuestand.latestDepositId',
  withdrawal: 'shuestand.latestWithdrawalId',
}
import { useQuery } from '@tanstack/react-query'
import './App.css'
import { config } from './config'
import { DELIVERY_TARGETS } from './config/deliveryTargets'
import { detectTokenMint } from './lib/cashu'
import type { CreateWithdrawalRequest } from './types/api'
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
  const [depositAmount, setDepositAmount] = useState(DEFAULT_AMOUNT)
  const [withdrawalAmount, setWithdrawalAmount] = useState(DEFAULT_AMOUNT)
  const [withdrawalMethod, setWithdrawalMethod] = useState<'token' | 'payment_request'>(
    'token'
  )
  const [deliveryTarget, setDeliveryTarget] = useState('manual')
  const [customDeliveryHint, setCustomDeliveryHint] = useState('')
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
    if (typeof window === 'undefined') {
      return
    }
    const storedDeposit = window.localStorage.getItem(STORAGE_KEYS.deposit)
    if (storedDeposit) {
      setLatestDepositId(storedDeposit)
    }
    const storedWithdrawal = window.localStorage.getItem(STORAGE_KEYS.withdrawal)
    if (storedWithdrawal) {
      setLatestWithdrawalId(storedWithdrawal)
    }
  }, [])

  useEffect(() => {
    if (flow !== 'withdrawal') {
      setTokenMintInfo(null)
    }
  }, [flow])

  const rememberDepositId = (id: string) => {
    setLatestDepositId(id)
    if (typeof window !== 'undefined') {
      window.localStorage.setItem(STORAGE_KEYS.deposit, id)
    }
  }

  const rememberWithdrawalId = (id: string) => {
    setLatestWithdrawalId(id)
    if (typeof window !== 'undefined') {
      window.localStorage.setItem(STORAGE_KEYS.withdrawal, id)
    }
  }

  const clearDepositId = () => {
    setLatestDepositId(null)
    if (typeof window !== 'undefined') {
      window.localStorage.removeItem(STORAGE_KEYS.deposit)
    }
  }

  const clearWithdrawalId = () => {
    setLatestWithdrawalId(null)
    if (typeof window !== 'undefined') {
      window.localStorage.removeItem(STORAGE_KEYS.withdrawal)
    }
  }

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
        const selectedTarget = DELIVERY_TARGETS.find((target) => target.id === deliveryTarget)
        const resolvedHint =
          deliveryTarget === 'custom'
            ? customDeliveryHint.trim()
            : selectedTarget?.hint ?? null
        const payload = {
          amount_sats: Number(depositAmount),
          metadata: { source: 'ui-proto' },
          delivery_hint: resolvedHint || undefined,
        }
        const deposit = await createDeposit(payload)
        rememberDepositId(deposit.id)
        setMessage(
          `Deposit ${deposit.id} → ${deposit.address} (${deposit.state})`
        )
      } else {
        const payload: CreateWithdrawalRequest = {
          amount_sats: Number(withdrawalAmount),
          delivery_address: deliveryAddress.trim(),
        }

        if (payload.amount_sats <= 0 || Number.isNaN(payload.amount_sats)) {
          throw new Error('Withdrawal amount must be greater than zero')
        }
        if (payload.amount_sats < config.withdrawalMinSats) {
          throw new Error(
            `Withdrawal amount must be at least ${config.withdrawalMinSats.toLocaleString()} sats`
          )
        }

        if (withdrawalMethod === 'token') {
          const trimmed = token.trim()
          if (!trimmed) {
            throw new Error('Provide a Cashu token or switch to payment requests')
          }
          payload.token = trimmed
        } else {
          payload.create_payment_request = true
        }

        const withdrawal = await createWithdrawal(payload)
        rememberWithdrawalId(withdrawal.id)
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
                      value={depositAmount}
                      onChange={(e) => setDepositAmount(e.target.value)}
                      required
                    />
                  </label>
                  <label>
                    Delivery target (optional)
                    <select
                      value={deliveryTarget}
                      onChange={(e) => setDeliveryTarget(e.target.value)}
                    >
                      {DELIVERY_TARGETS.map((target) => (
                        <option key={target.id} value={target.id}>
                          {target.label}
                        </option>
                      ))}
                    </select>
                    <span className="helper">
                      {
                        DELIVERY_TARGETS.find((target) => target.id === deliveryTarget)
                          ?.description
                      }
                    </span>
                  </label>
                  {deliveryTarget === 'custom' && (
                    <label>
                      Custom delivery URL
                      <input
                        type="text"
                        value={customDeliveryHint}
                        onChange={(e) => setCustomDeliveryHint(e.target.value)}
                        placeholder="cashu://wallet/… or https://webhook"
                      />
                    </label>
                  )}
                </>
              ) : (
                <>
                  <label>
                    Amount (sats)
                    <input
                      type="number"
                      min={config.withdrawalMinSats}
                      value={withdrawalAmount}
                      onChange={(e) => setWithdrawalAmount(e.target.value)}
                      required
                    />
                    <span className="helper">
                      Minimum {config.withdrawalMinSats.toLocaleString()} sats
                    </span>
                  </label>
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
                <>
                  <DepositStatusCard
                    deposit={latestDeposit}
                    error={normalizeError(depositError)}
                    isLoading={depositLoading}
                    hasSubmission={Boolean(latestDepositId)}
                  />
                  {latestDepositId && (
                    <button
                      type="button"
                      className="link-button"
                      onClick={clearDepositId}
                    >
                      Forget this deposit
                    </button>
                  )}
                </>
              ) : (
                <>
                  <WithdrawalStatusCard
                    withdrawal={latestWithdrawal}
                    error={normalizeError(withdrawalError)}
                    isLoading={withdrawalLoading}
                    hasSubmission={Boolean(latestWithdrawalId)}
                  />
                  {latestWithdrawalId && (
                    <button
                      type="button"
                      className="link-button"
                      onClick={clearWithdrawalId}
                    >
                      Forget this withdrawal
                    </button>
                  )}
                </>
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
