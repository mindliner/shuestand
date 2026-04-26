CREATE TABLE IF NOT EXISTS support_cases (
    session_id TEXT PRIMARY KEY,
    status TEXT NOT NULL DEFAULT 'open',
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    closed_at TEXT,
    CONSTRAINT support_cases_session_fk
      FOREIGN KEY (session_id) REFERENCES sessions(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_support_cases_status_updated
    ON support_cases (status, updated_at DESC);
