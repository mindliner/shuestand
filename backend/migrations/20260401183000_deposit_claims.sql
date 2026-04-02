-- One-time pickup/claim boundary for minted ecash

CREATE TABLE IF NOT EXISTS deposit_claims (
  deposit_id TEXT PRIMARY KEY REFERENCES deposits(id) ON DELETE CASCADE,
  minted_token TEXT NOT NULL,
  claimed_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

