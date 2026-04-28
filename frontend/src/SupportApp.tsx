import { useMemo, useState, type FormEvent } from 'react'
import { useLocation, useNavigate } from 'react-router-dom'
import { AppVersion } from './components/AppVersion'
import { submitSupportMessage } from './lib/api'
import type { Theme } from './lib/theme'

type SessionInfo = {
  id: string
  token: string
  claimCode: string
  expiresAt: string
}

const SESSION_STORAGE_KEY = 'shuestand.session'

function readSession(): SessionInfo | null {
  if (typeof window === 'undefined') return null
  try {
    const raw = window.localStorage.getItem(SESSION_STORAGE_KEY)
    if (!raw) return null
    const parsed = JSON.parse(raw) as SessionInfo
    if (!parsed?.id || !parsed?.token) return null
    return parsed
  } catch {
    return null
  }
}

type SupportAppProps = {
  theme: Theme
  onThemeSelect: (mode: Theme) => void
}

export function SupportApp({ theme, onThemeSelect }: SupportAppProps) {
  const navigate = useNavigate()
  const location = useLocation()
  const [text, setText] = useState('')
  const [busy, setBusy] = useState(false)
  const [status, setStatus] = useState<string | null>(null)
  const session = useMemo(() => readSession(), [])
  const reason = useMemo(() => new URLSearchParams(location.search).get('reason') ?? '', [location.search])

  const submit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault()
    if (!session?.token) {
      setStatus('No active session found. Please start or resume a session first.')
      return
    }
    const message = text.trim()
    if (!message) {
      setStatus('Please enter a message.')
      return
    }

    setBusy(true)
    setStatus(null)
    try {
      await submitSupportMessage(session.token, {
        message,
        context: reason ? { reason } : undefined,
      })
      setText('')
      setStatus('Message sent to the operator.')
    } catch (err) {
      setStatus(err instanceof Error ? err.message : String(err))
    } finally {
      setBusy(false)
    }
  }

  return (
    <main className="app-shell">
      <header>
        <div>
          <p className="eyebrow">shuestand · support</p>
          <h1>Support Case</h1>
          <p className="lede">If something went wrong, briefly describe what happened here.</p>
        </div>
        <div className="header-actions">
          <div className="theme-toggle" role="group" aria-label="Color theme">
            <button type="button" className={theme === 'light' ? 'active' : ''} onClick={() => onThemeSelect('light')}>Day</button>
            <button type="button" className={theme === 'dark' ? 'active' : ''} onClick={() => onThemeSelect('dark')}>Night</button>
          </div>
          <button type="button" className="link-button" onClick={() => navigate('/')}>Back to kiosk</button>
        </div>
      </header>

      <section className="panel">
        <div className="status-block">
          <h3>Message to operator</h3>
          <p className="status-meta code">Session: {session?.id ?? 'not available'}</p>
          {reason ? <p className="status-meta warning">Reason: {reason}</p> : null}
          <form onSubmit={submit}>
            <label>
              Message
              <textarea
                value={text}
                onChange={(e) => setText(e.target.value)}
                rows={7}
                maxLength={2048}
                placeholder="Please describe the issue and include TX/address/token reference if available"
              />
            </label>
            <button type="submit" disabled={busy}>{busy ? 'Sending…' : 'Send message'}</button>
          </form>
          {status ? <p className="message">{status}</p> : null}
        </div>
      </section>

      <AppVersion />
    </main>
  )
}
