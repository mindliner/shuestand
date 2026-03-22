const apiBase =
  import.meta.env.VITE_SHUESTAND_API_BASE ?? 'http://localhost:8080'
const cashuMintUrl = import.meta.env.VITE_SHUESTAND_MINT_URL?.trim() ?? ''

export const config = {
  apiBase,
  cashuMintUrl,
}
