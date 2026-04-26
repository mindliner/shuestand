CREATE TABLE IF NOT EXISTS support_messages (
    id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL,
    source TEXT NOT NULL,
    message TEXT NOT NULL,
    context JSONB,
    created_at TEXT NOT NULL,
    CONSTRAINT support_messages_session_fk
      FOREIGN KEY (session_id) REFERENCES sessions(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_support_messages_session_created
    ON support_messages (session_id, created_at DESC);
