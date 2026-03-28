import { bech32, bech32m } from '@scure/base'
import { sha256 } from '@noble/hashes/sha2.js'

const BASE58_ALPHABET = '123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz'
const BASE58_MAP: Record<string, number> = {}
for (let i = 0; i < BASE58_ALPHABET.length; i += 1) {
  BASE58_MAP[BASE58_ALPHABET[i]] = i
}

const decodeBase58 = (value: string): Uint8Array | null => {
  if (!value) {
    return null
  }
  let num = 0n
  for (const char of value) {
    const digit = BASE58_MAP[char]
    if (digit === undefined) {
      return null
    }
    num = num * 58n + BigInt(digit)
  }

  const bytes: number[] = []
  while (num > 0n) {
    bytes.push(Number(num % 256n))
    num /= 256n
  }
  bytes.reverse()

  const leadingZeros = value.match(/^1+/)?.[0].length ?? 0
  const result = new Uint8Array(leadingZeros + bytes.length)
  for (let i = 0; i < bytes.length; i += 1) {
    result[leadingZeros + i] = bytes[i]
  }
  return result
}

const doubleSha256 = (input: Uint8Array): Uint8Array => {
  const first = sha256(input)
  return sha256(first)
}

const isValidBase58Address = (value: string): boolean => {
  const decoded = decodeBase58(value)
  if (!decoded || decoded.length < 4) {
    return false
  }
  const payload = decoded.slice(0, -4)
  const checksum = decoded.slice(-4)
  const hash = doubleSha256(payload)
  for (let i = 0; i < 4; i += 1) {
    if (checksum[i] !== hash[i]) {
      return false
    }
  }
  // mainnet P2PKH starts with 0x00 ("1...") and P2SH with 0x05 ("3...")
  return payload[0] === 0x00 || payload[0] === 0x05
}

const isBech32MixedCase = (value: string): boolean => {
  const hasLower = value !== value.toUpperCase()
  const hasUpper = value !== value.toLowerCase()
  return hasLower && hasUpper
}

const isValidBech32Address = (value: string): boolean => {
  if (isBech32MixedCase(value)) {
    return false
  }
  const normalized = value.toLowerCase()
  if (!normalized.startsWith('bc1')) {
    return false
  }
  try {
    const decoder = normalized.startsWith('bc1p') ? bech32m : bech32
    const { prefix, words } = decoder.decode(normalized as `${string}1${string}`, 90)
    if (prefix !== 'bc' || words.length === 0) {
      return false
    }
    return true
  } catch {
    return false
  }
}

export const isValidBitcoinAddress = (value: string): boolean => {
  if (!value) {
    return false
  }
  const trimmed = value.trim()
  if (!trimmed) {
    return false
  }
  if (trimmed.startsWith('1') || trimmed.startsWith('3')) {
    return isValidBase58Address(trimmed)
  }
  if (trimmed.toLowerCase().startsWith('bc1')) {
    return isValidBech32Address(trimmed)
  }
  return false
}
