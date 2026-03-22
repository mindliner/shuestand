import { getDecodedToken } from '@cashu/cashu-ts'

const DEFAULT_ERROR = 'Unable to decode token'

const extractCashuToken = (raw: string): string | null => {
  const compact = raw.replace(/\s+/g, '')
  if (!compact) return null
  const idx = compact.indexOf('cashu')
  if (idx === -1) return null
  return compact.slice(idx)
}

export type DetectedTokenMint =
  | { mintUrl: string }
  | { error: string }
  | null

export const detectTokenMint = (rawToken: string): DetectedTokenMint => {
  const candidate = extractCashuToken(rawToken)
  if (!candidate) return null

  try {
    const decoded = getDecodedToken(candidate)
    const mintUrl = decoded?.mint?.trim()
    if (mintUrl) {
      return { mintUrl }
    }
    return { error: DEFAULT_ERROR }
  } catch (err) {
    console.error('Failed to decode Cashu token', err)
    const message = err instanceof Error ? err.message : DEFAULT_ERROR
    return { error: message.includes('Token version is not supported') ? 'Unsupported Cashu token encoding' : DEFAULT_ERROR }
  }
}
