import { Navigate, Route, Routes } from 'react-router-dom'
import './App.css'
import { KioskApp } from './KioskApp'
import { OperatorApp } from './OperatorApp'
import { SupportApp } from './SupportApp'

export default function App() {
  return (
    <Routes>
      <Route
        path="/"
        element={<KioskApp />}
      />
      <Route
        path="/operator"
        element={<OperatorApp />}
      />
      <Route
        path="/support"
        element={<SupportApp />}
      />
      <Route path="*" element={<Navigate to="/" replace />} />
    </Routes>
  )
}
