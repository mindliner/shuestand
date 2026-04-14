export function AppVersion() {
  return (
    <footer className="app-version-banner" aria-label="Build metadata">
      Shuestand v{__APP_VERSION__} · Alpha · {__APP_COMMIT__}
    </footer>
  )
}
