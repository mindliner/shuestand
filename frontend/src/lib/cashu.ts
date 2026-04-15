import { getTokenMetadata } from '@cashu/cashu-ts'

const DEFAULT_ERROR = 'Unable to decode token'

const extractCashuToken = (raw: string): string | null => {
  const compact = raw.replace(/\s+/g, '')
  if (!compact) return null
  const idx = compact.indexOf('cashu')
  if (idx === -1) return null
  return compact.slice(idx)
}

type AmountLike = number | bigint | { toString?: () => string } | null | undefined

const toNumericAmount = (amount: AmountLike): number => {
  if (amount == null) return 0
  if (typeof amount === 'number') return Number.isFinite(amount) ? amount : 0
  if (typeof amount === 'bigint') return Number(amount)
  if (typeof amount === 'object' && typeof amount.toString === 'function') {
    const value = Number(amount.toString())
    return Number.isFinite(value) ? value : 0
  }
  return 0
}

export type DetectedTokenMint =
  | { mintUrl: string; amount: number }
  | { error: string }
  | null

export const detectTokenMint = (rawToken: string): DetectedTokenMint => {
  const candidate = extractCashuToken(rawToken)
  if (!candidate) return null

  try {
    const metadata = getTokenMetadata(candidate)
    const mintUrl = metadata?.mint?.trim()
    if (!mintUrl) {
      return { error: DEFAULT_ERROR }
    }
    const amount = toNumericAmount(metadata.amount)
    if (!amount) {
      return { error: 'Could not decode the token value (the backend will still validate it)' }
    }
    return { mintUrl, amount }
  } catch (err) {
    console.error('Failed to decode Cashu token', err)
    const message = err instanceof Error ? err.message : DEFAULT_ERROR
    return { error: message.includes('Token version is not supported') ? 'Unsupported Cashu token encoding' : DEFAULT_ERROR }
  }
}
