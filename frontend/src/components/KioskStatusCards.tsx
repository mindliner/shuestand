import { useState } from 'react'
import { QRCodeSVG } from 'qrcode.react'
import type {
  Deposit,
  DepositState,
  Withdrawal,
  WithdrawalState,
} from '../types/api'

interface DepositStatusCardProps {
  deposit?: Deposit
  error: Error | null
  isLoading: boolean
  hasSubmission: boolean
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
}: DepositStatusCardProps) {
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

  return (
    <div className="status-block">
      <h3>Deposit progress</h3>
      {deposit.delivery_hint && (
        <span className="hint-badge">Delivery: {deposit.delivery_hint}</span>
      )}
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
      <p className="status-meta code">{deposit.address}</p>
      <CopyButton label="Copy address" text={deposit.address} />
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
      <StatusTimeline stages={DEPOSIT_STAGES} current={deposit.state} />
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

  return (
    <div className="status-block">
      <h3>Withdrawal progress</h3>
      <p className="status-line">
        <strong>{withdrawal.state}</strong>
      </p>
      <p className="status-meta code">
        #{withdrawal.id} → {withdrawal.delivery_address}
      </p>
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
      {withdrawal.txid && (
        <>
          <p className="status-meta code">tx: {withdrawal.txid}</p>
          <CopyButton label="Copy txid" text={withdrawal.txid} />
        </>
      )}
      <StatusTimeline stages={WITHDRAWAL_STAGES} current={withdrawal.state} />
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
    try {
      await navigator.clipboard.writeText(text)
      setState('copied')
    } catch (err) {
      console.error('copy failed', err)
      setState('error')
    } finally {
      setTimeout(() => setState('idle'), 1500)
    }
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

type Stage<T extends string> = {
  key: T
  label: string
  helper?: string
}

const DEPOSIT_STAGES: Stage<DepositState>[] = [
  { key: 'pending', label: 'Address allocated', helper: 'Waiting for funding tx' },
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

  return (
    <ol className="status-timeline">
      {stages.map((stage, idx) => {
        const stateClass =
          idx < activeIndex
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
