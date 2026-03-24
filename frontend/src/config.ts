const apiBase =
  import.meta.env.VITE_SHUESTAND_API_BASE ?? 'http://localhost:8080'
const cashuMintUrl = import.meta.env.VITE_SHUESTAND_MINT_URL?.trim() ?? ''
const withdrawalMinSats = Number(
  import.meta.env.VITE_WITHDRAWAL_MIN_SATS ?? '50000'
)
const depositMinSats = Number(
  import.meta.env.VITE_DEPOSIT_MIN_SATS ?? '50000'
)

export const config = {
  apiBase,
  cashuMintUrl,
  withdrawalMinSats,
  depositMinSats,
}
