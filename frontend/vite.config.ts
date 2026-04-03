import { readFileSync } from 'node:fs'
import { execSync } from 'node:child_process'
import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'

const pkg = JSON.parse(
  readFileSync(new URL('./package.json', import.meta.url), 'utf-8')
)

const backendVersion = (() => {
  try {
    const backendManifest = readFileSync(
      new URL('../backend/Cargo.toml', import.meta.url),
      'utf-8'
    )
    const match = backendManifest.match(/^\s*version\s*=\s*"([^"]+)"/m)
    return match?.[1] ?? pkg.version ?? '0.0.0'
  } catch {
    return pkg.version ?? '0.0.0'
  }
})()

const commitHash = (() => {
  try {
    return execSync('git rev-parse --short HEAD').toString().trim()
  } catch {
    return 'unknown'
  }
})()

// https://vite.dev/config/
export default defineConfig({
  plugins: [react()],
  define: {
    __APP_VERSION__: JSON.stringify(backendVersion),
    __APP_COMMIT__: JSON.stringify(commitHash),
  },
  server: {
    host: '0.0.0.0',
    port: 5173,
    allowedHosts: ['logom-holo'],
  },
  preview: {
    host: '0.0.0.0',
    port: 4173,
    allowedHosts: ['logom-holo'],
  },
})
