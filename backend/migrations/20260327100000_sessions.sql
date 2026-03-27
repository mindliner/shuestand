-- Session tracking for kiosk users
CREATE TABLE sessions (
    id TEXT PRIMARY KEY,
    token_hash TEXT NOT NULL UNIQUE,
    created_at TEXT NOT NULL,
    last_seen_at TEXT NOT NULL,
    expires_at TEXT NOT NULL
);

ALTER TABLE deposits ADD COLUMN session_id TEXT REFERENCES sessions(id);
ALTER TABLE withdrawals ADD COLUMN session_id TEXT REFERENCES sessions(id);
