import { useEffect, useRef, useState } from 'react'
import { QRCodeSVG } from 'qrcode.react'
import { ApiClientError } from '../lib/api'
import { copyTextWithFallback } from '../lib/clipboard'
import type {
  Deposit,
  DepositState,
  Withdrawal,
  WithdrawalState,
} from '../types/api'

const AVERAGE_BLOCK_MINUTES = 10

interface DepositStatusCardProps {
  deposit?: Deposit
  error: Error | null
  isLoading: boolean
  hasSubmission: boolean
  pendingDepositTtlSecs?: number
  pickupToken?: string | null
  revealedToken?: string | null
  onPickup?: () => void
  pickupPending?: boolean
  pickupError: Error | null
  onClear?: (deposit: Deposit) => void
}

interface WithdrawalStatusCardProps {
  withdrawal?: Withdrawal
  error: Error | null
  isLoading: boolean
  hasSubmission: boolean
}

export function DepositStatusCard({
  deposit,
  error,
  isLoading,
  hasSubmission,
  pendingDepositTtlSecs = 600,
  pickupToken,
  revealedToken,
  onPickup,
  pickupPending = false,
  pickupError,
  onClear,
}: DepositStatusCardProps) {
  const previousDepositId = useRef<string | null>(null)
  const previousDepositState = useRef<DepositState | null>(null)
  const [nowMs, setNowMs] = useState(() => Date.now())

  useEffect(() => {
    if (!deposit) {
      previousDepositId.current = null
      previousDepositState.current = null
      return
    }

    const isSameDeposit = previousDepositId.current === deposit.id
    const transitionedToReady =
      deposit.state === 'ready' &&
      (!isSameDeposit || previousDepositState.current !== 'ready')

    if (transitionedToReady) {
      notifyDepositReady()
    }

    previousDepositId.current = deposit.id
    previousDepositState.current = deposit.state
  }, [deposit])

  useEffect(() => {
    if (typeof window === 'undefined') {
      return
    }
    const handle = window.setInterval(() => {
      setNowMs(Date.now())
    }, 1000)
    return () => {
      window.clearInterval(handle)
    }
  }, [])

  if (!hasSubmission) {
    return (
      <div className="status-block">
        <h3>Deposit progress</h3>
        <p>No deposit submitted yet</p>
      </div>
    )
  }

  if (error) {
    return (
      <div className="status-block">
        <h3>Deposit progress</h3>
        <p className="status-error">{error.message}</p>
      </div>
    )
  }

  if (!deposit) {
    return (
      <div className="status-block">
        <h3>Deposit progress</h3>
        <p>{isLoading ? 'Loading…' : 'Fetching latest deposit...'}</p>
      </div>
    )
  }

  const bip21 = buildBip21Uri(deposit.address, deposit.amount_sats)
  const awaitingConfirmations =
    deposit.state === 'pending' ||
    deposit.state === 'partial_payment_received' ||
    deposit.state === 'confirming'
  const confirmationEtaText = awaitingConfirmations
    ? getConfirmationEtaText(deposit) ?? 'waiting for final block'
    : null

  const maxStaticQrChars = 900
  const canRenderStaticPickupQr = Boolean(
    revealedToken && revealedToken.length <= maxStaticQrChars
  )
  const topUpWindow = getTopUpWindow(deposit, pendingDepositTtlSecs, nowMs)

  return (
    <div className="status-block">
      <h3>Deposit progress</h3>
      <p className="status-meta code">{deposit.id}</p>
      <CopyButton label="Copy deposit ID" text={deposit.id} />
      {bip21 && (
        <div className="qr-card">
          <QRCodeSVG value={bip21} size={132} />
          <CopyButton label="Copy payment URI" text={bip21} />
        </div>
      )}
      <p className="status-line">
        <strong>{deposit.state}</strong> {deposit.confirmations}/
        {deposit.target_confirmations} confs
      </p>
      {deposit.state === 'partial_payment_received' && (
        <p className="status-error">
          Partial payment received. Send the remaining sats to this same address, then wait for confirmations.
        </p>
      )}
      {topUpWindow &&
        (topUpWindow.expired ? (
          <p className="status-meta warning">
            Top-up window expired. This deposit will be marked failed automatically.
          </p>
        ) : (
          <p className="status-meta warning">
            Top-up window: {topUpWindow.label} remaining.
          </p>
        ))}
      {confirmationEtaText && (
        <p className="status-meta">
          Estimated time to mint: {confirmationEtaText}. We'll notify this device when the token is ready.
        </p>
      )}
      <p className="status-meta code">{deposit.address}</p>
      <CopyButton label="Copy address" text={deposit.address} />
      {deposit.state === 'ready' && (
        <div className="status-block nested">
          <p>Token is minted. Press the button when you're ready to claim it.</p>
          {pickupToken && onPickup ? (
            <button type="button" onClick={onPickup} disabled={pickupPending}>
              {pickupPending ? 'Revealing…' : 'Reveal token'}
            </button>
          ) : (
            <p className="status-error">Pickup token unavailable for this deposit.</p>
          )}
          {pickupError && (
            <p className="status-error">
              {pickupError instanceof ApiClientError && pickupError.code === 'deposit_not_ready_for_pickup'
                ? 'Token already revealed for this deposit. Check your clipboard; new reveals are blocked for safety.'
                : pickupError.message}
            </p>
          )}
        </div>
      )}
      {revealedToken && (
        <div className="token-card">
          <p>Pickup token</p>
          {canRenderStaticPickupQr ? (
            <div className="qr-card">
              <QRCodeSVG value={revealedToken} size={132} />
            </div>
          ) : (
            <p className="status-meta">
              Token is too large for a reliable static QR code on this display. Use copy instead.
            </p>
          )}
          <CopyButton label="Copy token" text={revealedToken} />
          <p className="status-meta">
            We auto-copied to clipboard; this button is a backup if the clipboard is cleared.
          </p>
        </div>
      )}
      {deposit.txid && (
        <>
          <p className="status-meta code">tx: {deposit.txid}</p>
          <CopyButton label="Copy txid" text={deposit.txid} />
        </>
      )}
      {deposit.token && (
        <div className="token-card">
          <p>Token ready</p>
          <p className="status-meta code">{deposit.token}</p>
          <CopyButton label="Copy token" text={deposit.token} />
        </div>
      )}
      <StatusTimeline
        stages={
          deposit.state === 'failed'
            ? DEPOSIT_STAGES
            : DEPOSIT_STAGES.filter((stage) => stage.key !== 'failed')
        }
        current={deposit.state}
      />
      {onClear && (
        <button
          type="button"
          className="link-button"
          onClick={() => onClear(deposit)}
        >
          Archive this deposit
        </button>
      )}
    </div>
  )
}

export function WithdrawalStatusCard({
  withdrawal,
  error,
  isLoading,
  hasSubmission,
}: WithdrawalStatusCardProps) {
  if (!hasSubmission) {
    return (
      <div className="status-block">
        <h3>Withdrawal progress</h3>
        <p>No withdrawal submitted yet</p>
      </div>
    )
  }

  if (error) {
    return (
      <div className="status-block">
        <h3>Withdrawal progress</h3>
        <p className="status-error">{error.message}</p>
      </div>
    )
  }

  if (!withdrawal) {
    return (
      <div className="status-block">
        <h3>Withdrawal progress</h3>
        <p>{isLoading ? 'Loading…' : 'Fetching latest withdrawal...'}</p>
      </div>
    )
  }

  const paymentRequest = withdrawal.payment_request ?? null
  const paymentExpiresAt = paymentRequest?.expires_at
    ? new Date(paymentRequest.expires_at)
    : null
  const paymentFulfilledAt = paymentRequest?.fulfilled_at
    ? new Date(paymentRequest.fulfilled_at)
    : null
  const isAwaitingPayment = withdrawal.state === 'funding' && Boolean(paymentRequest)

  return (
    <div className="status-block">
      <h3>Withdrawal progress</h3>
      <p className="status-line">
        <strong>{withdrawal.state}</strong>
      </p>
      <p className="status-meta code">
        #{withdrawal.id} → {withdrawal.delivery_address}
      </p>
      {withdrawal.error && (
        <p className="status-error">
          {withdrawal.error}
        </p>
      )}
      <CopyButton label="Copy destination" text={withdrawal.delivery_address} />
      {withdrawal.source_mint_url && (
        <p className={`status-meta ${withdrawal.is_foreign_mint ? 'warning' : ''}`}>
          Mint: {withdrawal.source_mint_url}
          {withdrawal.is_foreign_mint
            ? ' · foreign token — swapping to kiosk mint first'
            : ''}
        </p>
      )}
      {typeof withdrawal.swap_fee_sats === 'number' && withdrawal.swap_fee_sats > 0 && (
        <p className="status-meta warning">
          Swap fee: {withdrawal.swap_fee_sats} sats (kept by swap/melt)
        </p>
      )}
      {typeof withdrawal.requested_amount_sats === 'number' && withdrawal.requested_amount_sats > 0 && (
        <p className="status-meta">Requested: {withdrawal.requested_amount_sats} sats</p>
      )}
      {typeof withdrawal.token_value_sats === 'number' && withdrawal.token_value_sats > 0 && (
        <p className="status-meta">Redeemed: {withdrawal.token_value_sats} sats</p>
      )}
      {withdrawal.txid && (
        <>
          <p className="status-meta code">tx: {withdrawal.txid}</p>
          <CopyButton label="Copy txid" text={withdrawal.txid} />
        </>
      )}

      {isAwaitingPayment && paymentRequest && (
        <div className="status-block nested">
          <p>Scan this Cashu payment request to fund the withdrawal.</p>
          <div className="qr-card">
            <QRCodeSVG value={paymentRequest.creq} size={132} />
            <CopyButton label="Copy payment request" text={paymentRequest.creq} />
          </div>
          {paymentExpiresAt && (
            <p className="status-meta">
              Expires at {paymentExpiresAt.toLocaleTimeString()} ({
                Math.max(
                  0,
                  Math.floor((paymentExpiresAt.getTime() - Date.now()) / 1000)
                )
              }{' '}
              s left)
            </p>
          )}
        </div>
      )}

      {!isAwaitingPayment && paymentRequest && paymentFulfilledAt && (
        <p className="status-meta success">
          Payment received at {paymentFulfilledAt.toLocaleTimeString()}
        </p>
      )}

      <StatusTimeline
        stages={
          withdrawal.state === 'failed'
            ? WITHDRAWAL_STAGES
            : WITHDRAWAL_STAGES.filter((stage) => stage.key !== 'failed')
        }
        current={withdrawal.state}
      />
    </div>
  )
}

interface CopyButtonProps {
  text: string
  label: string
}

export function CopyButton({ text, label }: CopyButtonProps) {
  const [state, setState] = useState<'idle' | 'copied' | 'error'>('idle')

  const handleCopy = async () => {
    const success = await copyTextWithFallback(text)
    setState(success ? 'copied' : 'error')
    setTimeout(() => setState('idle'), 1500)
  }

  return (
    <button
      type="button"
      className={`copy-btn ${state}`}
      onClick={handleCopy}
      aria-label={label}
    >
      {state === 'copied' ? 'Copied!' : label}
    </button>
  )
}

const buildBip21Uri = (address: string, amountSats?: number) => {
  if (!amountSats || amountSats <= 0) {
    return `bitcoin:${address}`
  }
  const btc = (amountSats / 1e8).toFixed(8).replace(/\.0+$/, '')
  return `bitcoin:${address}?amount=${btc}`
}

const getConfirmationEtaText = (deposit: Deposit): string | null => {
  const remaining = Math.max(
    0,
    (deposit?.target_confirmations ?? 0) - (deposit?.confirmations ?? 0)
  )

  if (remaining <= 0) {
    return null
  }

  const minutes = remaining * AVERAGE_BLOCK_MINUTES

  if (minutes < 1) {
    return 'less than a minute'
  }

  if (minutes < 60) {
    return `≈ ${Math.max(1, Math.round(minutes))} min`
  }

  const hours = minutes / 60

  if (hours < 24) {
    return `≈ ${hours.toFixed(1)} h`
  }

  const days = hours / 24
  return `≈ ${days.toFixed(1)} d`
}

const getTopUpWindow = (
  deposit: Deposit,
  ttlSecs: number,
  nowMs: number
): { expired: boolean; label: string } | null => {
  if (!Number.isFinite(ttlSecs) || ttlSecs <= 0) {
    return null
  }
  if (deposit.state !== 'pending' && deposit.state !== 'partial_payment_received') {
    return null
  }
  if (!deposit.created_at) {
    return null
  }
  const createdAtMs = Date.parse(deposit.created_at)
  if (!Number.isFinite(createdAtMs)) {
    return null
  }

  const remainingSecs = Math.ceil((createdAtMs + ttlSecs * 1000 - nowMs) / 1000)
  if (remainingSecs <= 0) {
    return { expired: true, label: '0s' }
  }

  return {
    expired: false,
    label: formatRemaining(remainingSecs),
  }
}

const formatRemaining = (totalSeconds: number): string => {
  const hours = Math.floor(totalSeconds / 3600)
  const minutes = Math.floor((totalSeconds % 3600) / 60)
  const seconds = totalSeconds % 60

  if (hours > 0) {
    return `${hours}h ${String(minutes).padStart(2, '0')}m ${String(seconds).padStart(2, '0')}s`
  }
  return `${minutes}m ${String(seconds).padStart(2, '0')}s`
}

const notifyDepositReady = () => {
  if (typeof window === 'undefined') {
    return
  }

  if (typeof navigator !== 'undefined' && typeof navigator.vibrate === 'function') {
    try {
      navigator.vibrate([160, 100, 160])
    } catch {
      // ignore vibration errors
    }
  }

  const AudioCtor =
    window.AudioContext || (window as typeof window & { webkitAudioContext?: typeof AudioContext }).webkitAudioContext

  if (!AudioCtor) {
    return
  }

  try {
    const context = new AudioCtor()
    const oscillator = context.createOscillator()
    const gain = context.createGain()
    oscillator.type = 'triangle'
    oscillator.frequency.value = 880
    gain.gain.value = 0.05
    oscillator.connect(gain)
    gain.connect(context.destination)
    oscillator.start()

    setTimeout(() => {
      oscillator.stop()
      context.close().catch(() => {})
    }, 500)
  } catch {
    // ignore audio errors
  }
}

type Stage<T extends string> = {
  key: T
  label: string
  helper?: string
}

const DEPOSIT_STAGES: Stage<DepositState>[] = [
  { key: 'pending', label: 'Address allocated', helper: 'Waiting for funding tx' },
  {
    key: 'partial_payment_received',
    label: 'Partial payment received',
    helper: 'Send remaining sats to the same address',
  },
  {
    key: 'confirming',
    label: 'Confirmations in progress',
    helper: 'Needs on-chain depth',
  },
  { key: 'minting', label: 'Minting ecash proofs' },
  { key: 'delivering', label: 'Delivering token to hint' },
  { key: 'ready', label: 'Token ready for pickup' },
  { key: 'fulfilled', label: 'Token claimed' },
  { key: 'failed', label: 'Failed', helper: 'Operator action required' },
]

const WITHDRAWAL_STAGES: Stage<WithdrawalState>[] = [
  { key: 'funding', label: 'Awaiting ecash', helper: 'Scan the payment request' },
  { key: 'queued', label: 'Queued', helper: 'Waiting for worker' },
  { key: 'broadcasting', label: 'Redeeming + broadcasting' },
  { key: 'confirming', label: 'Awaiting confirmations' },
  { key: 'settled', label: 'Settled' },
  { key: 'failed', label: 'Failed', helper: 'Operator action required' },
]

interface StatusTimelineProps<T extends string> {
  stages: Stage<T>[]
  current: T
}

function StatusTimeline<T extends string>({ stages, current }: StatusTimelineProps<T>) {
  const currentIndex = stages.findIndex((stage) => stage.key === current)
  const activeIndex = currentIndex === -1 ? 0 : currentIndex
  const isFinalStage = currentIndex !== -1 && activeIndex === stages.length - 1

  return (
    <ol className="status-timeline">
      {stages.map((stage, idx) => {
        const stateClass =
          idx < activeIndex || (isFinalStage && idx === activeIndex)
            ? 'complete'
            : idx === activeIndex
              ? 'active'
              : 'upcoming'
        return (
          <li key={stage.key} className={`status-step ${stateClass}`}>
            <span className="status-bullet" />
            <div>
              <p className="status-step-label">{stage.label}</p>
              {stage.helper && (
                <p className="status-step-helper">{stage.helper}</p>
              )}
            </div>
          </li>
        )
      })}
    </ol>
  )
}
