-- Add pickup token gating for deposits
ALTER TABLE deposits ADD COLUMN pickup_token TEXT NOT NULL DEFAULT 'pickup-token-legacy';
ALTER TABLE deposits ADD COLUMN pickup_revealed_at TEXT;
UPDATE deposits SET pickup_token = CONCAT('legacy-', id) WHERE pickup_token = 'pickup-token-legacy';
ALTER TABLE deposits ALTER COLUMN pickup_token DROP DEFAULT;
