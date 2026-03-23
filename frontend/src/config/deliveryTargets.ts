export type DeliveryTarget = {
  id: string
  label: string
  description: string
  hint: string | null
}

export const DELIVERY_TARGETS: DeliveryTarget[] = [
  {
    id: 'manual',
    label: 'Show token / QR only',
    description: 'Keep the token on screen so you can copy or scan it into any wallet.',
    hint: null,
  },
  {
    id: 'minibits',
    label: 'Minibits on this device',
    description: 'Use the cashu:// wallet handler so the Minibits app can claim the token directly.',
    hint: 'cashu://wallet/minibits',
  },
  {
    id: 'macadamia',
    label: 'Macadamia (iOS)',
    description: 'Open the macadamia Cashu wallet on iPhone/iPad via the cashu:// handler.',
    hint: 'cashu://wallet/macadamia',
  },
  {
    id: 'custom',
    label: 'Custom URL (advanced)',
    description: 'Provide a webhook or wallet-specific URL yourself.',
    hint: null,
  },
]
