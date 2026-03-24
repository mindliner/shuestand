import type { FormEvent } from 'react'
import { useEffect, useState } from 'react'

const STORAGE_KEYS = {
  deposit: 'shuestand.latestDepositId',
  withdrawal: 'shuestand.latestWithdrawalId',
  deliveryAddress: 'shuestand.latestDeliveryAddress',
}
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import './App.css'
import { config } from './config'
import { copyTextWithFallback } from './lib/clipboard'
import { DELIVERY_TARGETS } from './config/deliveryTargets'
import { detectTokenMint } from './lib/cashu'
import type { CreateWithdrawalRequest } from './types/api'
import {
  ApiClientError,
  createDeposit,
  createWithdrawal,
  getDeposit,
  getWithdrawal,
  pickupDeposit,
} from './lib/api'
import {
  DepositStatusCard,
  WithdrawalStatusCard,
} from './components/KioskStatusCards'
import { OperatorPanel } from './components/OperatorPanel'

type Flow = 'deposit' | 'withdrawal'
type ViewMode = 'kiosk' | 'operator'
type TokenMintInfo =
  | { mintUrl: string; isForeign: boolean; amount: number }
  | { error: string }

const DEFAULT_DEPOSIT_AMOUNT = config.depositMinSats.toString()
const DEFAULT_WITHDRAWAL_AMOUNT = config.withdrawalMinSats.toString()
const STATUS_REFRESH_MS = 5000

const normalizeError = (err: unknown): Error | null => {
  if (!err) return null
  return err instanceof Error ? err : new Error(String(err))
}

export default function App() {
  const [view, setView] = useState<ViewMode>('kiosk')
  const [flow, setFlow] = useState<Flow>('deposit')
  const [depositAmount, setDepositAmount] = useState(DEFAULT_DEPOSIT_AMOUNT)
  const [withdrawalAmount, setWithdrawalAmount] = useState(DEFAULT_WITHDRAWAL_AMOUNT)
  const [withdrawalMethod, setWithdrawalMethod] = useState<'token' | 'payment_request'>(
    'token'
  )
  const [deliveryTarget, setDeliveryTarget] = useState('manual')
  const [customDeliveryHint, setCustomDeliveryHint] = useState('')
  const [token, setToken] = useState('')
  const [tokenMintInfo, setTokenMintInfo] = useState<TokenMintInfo | null>(null)
  const [deliveryAddress, setDeliveryAddress] = useState('')
  const [isSubmitting, setSubmitting] = useState(false)
  const [message, setMessage] = useState<string | null>(null)
  const [latestDepositId, setLatestDepositId] = useState<string | null>(null)
  const [latestDepositPickupToken, setLatestDepositPickupToken] = useState<string | null>(null)
  const [latestWithdrawalId, setLatestWithdrawalId] = useState<string | null>(
    null
  )

  const queryClient = useQueryClient()

  useEffect(() => {
    if (typeof window === 'undefined') {
      return
    }
    const storedDeposit = window.localStorage.getItem(STORAGE_KEYS.deposit)
    if (storedDeposit) {
      try {
        const parsed = JSON.parse(storedDeposit)
        if (parsed && typeof parsed === 'object' && typeof parsed.id === 'string') {
          setLatestDepositId(parsed.id)
          if (typeof parsed.pickupToken === 'string') {
            setLatestDepositPickupToken(parsed.pickupToken)
          } else {
            setLatestDepositPickupToken(null)
          }
        } else {
          setLatestDepositId(storedDeposit)
          setLatestDepositPickupToken(null)
        }
      } catch {
        setLatestDepositId(storedDeposit)
        setLatestDepositPickupToken(null)
      }
    }
    const storedWithdrawal = window.localStorage.getItem(STORAGE_KEYS.withdrawal)
    if (storedWithdrawal) {
      setLatestWithdrawalId(storedWithdrawal)
    }
    const storedDeliveryAddress = window.localStorage.getItem(
      STORAGE_KEYS.deliveryAddress
    )
    if (storedDeliveryAddress) {
      setDeliveryAddress(storedDeliveryAddress)
    }
  }, [])

  useEffect(() => {
    if (flow !== 'withdrawal') {
      setTokenMintInfo(null)
    }
  }, [flow])

  const rememberDeposit = (id: string, pickupToken: string) => {
    setLatestDepositId(id)
    setLatestDepositPickupToken(pickupToken)
    if (typeof window !== 'undefined') {
      window.localStorage.setItem(
        STORAGE_KEYS.deposit,
        JSON.stringify({ id, pickupToken })
      )
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
    setLatestDepositPickupToken(null)
    pickupMutation.reset()
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

  const handleDepositPickup = () => {
    if (!latestDepositId || !latestDepositPickupToken) {
      return
    }
    pickupMutation.mutate({ id: latestDepositId, pickupToken: latestDepositPickupToken })
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

  const pickupMutation = useMutation({
    mutationFn: ({ id, pickupToken }: { id: string; pickupToken: string }) =>
      pickupDeposit(id, pickupToken),
    onSuccess: async (resp) => {
      if (resp?.token) {
        const copied = await copyTextWithFallback(resp.token)
        if (copied) {
          setMessage('Token revealed and copied to clipboard')
        } else {
          setMessage('Token revealed (copy failed, use the copy button)')
        }
      } else {
        setMessage('Token revealed')
      }
      if (latestDepositId) {
        queryClient.invalidateQueries({ queryKey: ['deposit', latestDepositId] })
      }
    },
    onError: (err: unknown) => {
      const normalized = normalizeError(err)
      if (normalized) {
        setMessage(normalized.message)
      }
    },
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
    setTokenMintInfo({ mintUrl: detected.mintUrl, isForeign, amount: detected.amount })
  }

  const handleSubmit = async (evt: FormEvent<HTMLFormElement>) => {
    evt.preventDefault()
    setSubmitting(true)
    setMessage(null)

    try {
      if (flow === 'deposit') {
        const requestedAmount = Number(depositAmount)
        if (!Number.isFinite(requestedAmount) || requestedAmount < config.depositMinSats) {
          throw new Error(
            `Deposit amount must be at least ${config.depositMinSats.toLocaleString()} sats`
          )
        }
        const selectedTarget = DELIVERY_TARGETS.find((target) => target.id === deliveryTarget)
        const resolvedHint =
          deliveryTarget === 'custom'
            ? customDeliveryHint.trim()
            : selectedTarget?.hint ?? null
        const payload = {
          amount_sats: requestedAmount,
          metadata: { source: 'ui-proto' },
          delivery_hint: resolvedHint || undefined,
        }
        const creation = await createDeposit(payload)
        rememberDeposit(creation.deposit.id, creation.pickup_token)
        setMessage(
          `Deposit ${creation.deposit.id} → ${creation.deposit.address} (${creation.deposit.state})`
        )
      } else {
        let resolvedAmount = Number(withdrawalAmount)

        if (withdrawalMethod === 'token') {
          if (!tokenMintInfo || 'error' in tokenMintInfo) {
            throw new Error('Paste a valid Cashu token before submitting')
          }
          resolvedAmount = tokenMintInfo.amount
          if (resolvedAmount < config.withdrawalMinSats) {
            throw new Error(
              `Token value must be at least ${config.withdrawalMinSats.toLocaleString()} sats`
            )
          }
        } else {
          if (resolvedAmount <= 0 || Number.isNaN(resolvedAmount)) {
            throw new Error('Withdrawal amount must be greater than zero')
          }
          if (resolvedAmount < config.withdrawalMinSats) {
            throw new Error(
              `Withdrawal amount must be at least ${config.withdrawalMinSats.toLocaleString()} sats`
            )
          }
        }

        const payload: CreateWithdrawalRequest = {
          amount_sats: resolvedAmount,
          delivery_address: deliveryAddress.trim(),
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

  const withdrawalMinimum = config.withdrawalMinSats
  const tokenBelowMinimum = Boolean(
    tokenMintInfo &&
      !('error' in tokenMintInfo) &&
      tokenMintInfo.amount < withdrawalMinimum
  )

  const pickupError = pickupMutation.isError
    ? normalizeError(pickupMutation.error)
    : null

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
                      min={config.depositMinSats}
                      value={depositAmount}
                      onChange={(e) => setDepositAmount(e.target.value)}
                      required
                    />
                    <span className="helper">Minimum {config.depositMinSats.toLocaleString()} sats</span>
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
                        min={config.withdrawalMinSats}
                        value={withdrawalAmount}
                        onChange={(e) => setWithdrawalAmount(e.target.value)}
                        required={withdrawalMethod === 'payment_request'}
                      />
                      <span className="helper">
                        Minimum {config.withdrawalMinSats.toLocaleString()} sats
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
                      {tokenMintInfo &&
                        ('error' in tokenMintInfo ? (
                          <span className="helper warning">{tokenMintInfo.error}</span>
                        ) : (
                          <span
                            className={`helper ${
                              tokenMintInfo.isForeign || tokenBelowMinimum ? 'warning' : 'success'
                            }`}
                          >
                            {tokenBelowMinimum
                              ? `Token value is ${tokenMintInfo.amount.toLocaleString()} sats, but withdrawals require at least ${withdrawalMinimum.toLocaleString()} sats.`
                              : tokenMintInfo.isForeign
                                ? `Foreign token detected (${tokenMintInfo.mintUrl}); will be swapped to the Shuestand mint first. Value: ${tokenMintInfo.amount.toLocaleString()} sats.`
                                : `Mint detected: ${tokenMintInfo.mintUrl}. Value: ${tokenMintInfo.amount.toLocaleString()} sats.`}
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
                      onChange={(e) => handleDeliveryAddressChange(e.target.value)}
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
                    pickupToken={latestDepositPickupToken}
                    onPickup={handleDepositPickup}
                    pickupPending={pickupMutation.isPending}
                    pickupError={pickupError}
                    onClear={clearDepositId}
                  />
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
