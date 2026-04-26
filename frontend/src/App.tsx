import { useEffect, useState } from 'react'
import { Navigate, Route, Routes } from 'react-router-dom'
import './App.css'
import { KioskApp } from './KioskApp'
import { OperatorApp } from './OperatorApp'
import { SupportApp } from './SupportApp'
import { detectPreferredTheme, storeThemePreference, type Theme } from './lib/theme'

export default function App() {
  const [theme, setTheme] = useState<Theme>(() => detectPreferredTheme())

  useEffect(() => {
    if (typeof document !== 'undefined') {
      document.documentElement.setAttribute('data-theme', theme)
    }
    storeThemePreference(theme)
  }, [theme])

  const handleThemeSelect = (mode: Theme) => {
    setTheme(mode)
  }

  return (
    <Routes>
      <Route
        path="/"
        element={<KioskApp theme={theme} onThemeSelect={handleThemeSelect} />}
      />
      <Route
        path="/operator"
        element={<OperatorApp theme={theme} onThemeSelect={handleThemeSelect} />}
      />
      <Route
        path="/support"
        element={<SupportApp theme={theme} onThemeSelect={handleThemeSelect} />}
      />
      <Route path="*" element={<Navigate to="/" replace />} />
    </Routes>
  )
}
