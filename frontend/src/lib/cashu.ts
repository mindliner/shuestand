import { getDecodedToken } from '@cashu/cashu-ts'

const DEFAULT_ERROR = 'Unable to decode token'

const extractCashuToken = (raw: string): string | null => {
  const compact = raw.replace(/\s+/g, '')
  if (!compact) return null
  const idx = compact.indexOf('cashu')
  if (idx === -1) return null
  return compact.slice(idx)
}

type ProofLike = { amount?: number }

type TokenEntryLike = {
  mint?: string
  proofs?: ProofLike[]
}

type DecodedTokenLike = {
  mint?: string
  proofs?: ProofLike[]
  token?: TokenEntryLike[]
}

const sumProofList = (proofs?: ProofLike[]): number => {
  if (!proofs?.length) return 0
  return proofs.reduce((proofTotal, proof) => {
    const value = Number(proof?.amount ?? 0)
    return proofTotal + (Number.isFinite(value) ? value : 0)
  }, 0)
}

const sumProofAmounts = (decoded: DecodedTokenLike | null): number => {
  if (!decoded) return 0
  const nestedSum = decoded.token?.reduce((total, entry) => {
    return total + sumProofList(entry?.proofs)
  }, 0) ?? 0
  return sumProofList(decoded.proofs) + nestedSum
}

const resolveMintUrl = (decoded: DecodedTokenLike | null): string | null => {
  if (!decoded) return null
  const rootMint = decoded.mint?.trim()
  if (rootMint) return rootMint
  const nestedMint = decoded.token?.find((entry) => entry?.mint)?.mint?.trim()
  return nestedMint || null
}

export type DetectedTokenMint =
  | { mintUrl: string; amount: number }
  | { error: string }
  | null

export const detectTokenMint = (rawToken: string): DetectedTokenMint => {
  const candidate = extractCashuToken(rawToken)
  if (!candidate) return null

  try {
    const decoded = getDecodedToken(candidate) as DecodedTokenLike | null
    const mintUrl = resolveMintUrl(decoded)
    if (!mintUrl) {
      return { error: DEFAULT_ERROR }
    }
    const amount = sumProofAmounts(decoded)
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
