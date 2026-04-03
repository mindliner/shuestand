import { useNavigate } from 'react-router-dom'
import { OperatorPanel } from './components/OperatorPanel'
import { AppVersion } from './components/AppVersion'
import type { Theme } from './lib/theme'

type OperatorAppProps = {
  theme: Theme
  onThemeSelect: (mode: Theme) => void
}

export function OperatorApp({ theme, onThemeSelect }: OperatorAppProps) {
  const navigate = useNavigate()

  return (
    <main className="app-shell">
      <header>
        <div>
          <p className="eyebrow">shuestand · operator console</p>
          <h1>Operator console</h1>
          <p className="lede">
            Inspect wallet balances, rescan Electrum, and resolve stuck deposits/withdrawals
            without joining a kiosk session.
          </p>
        </div>
        <div className="header-actions">
          <div className="theme-toggle" role="group" aria-label="Color theme">
            <button
              type="button"
              className={theme === 'light' ? 'active' : ''}
              onClick={() => onThemeSelect('light')}
            >
              Day
            </button>
            <button
              type="button"
              className={theme === 'dark' ? 'active' : ''}
              onClick={() => onThemeSelect('dark')}
            >
              Night
            </button>
          </div>
          <div className="view-toggle">
            <button type="button" onClick={() => navigate('/')}>Kiosk</button>
            <button type="button" className="active">
              Operator
            </button>
          </div>
        </div>
      </header>

      <section className="panel operator-mode">
        <div className="operator-wrapper">
          <OperatorPanel />
        </div>
      </section>
      <AppVersion />
    </main>
  )
}
