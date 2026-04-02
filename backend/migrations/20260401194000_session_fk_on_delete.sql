-- Allow sessions to be deleted even when deposits/withdrawals still reference them
-- by nulling out the session_id column automatically.

ALTER TABLE deposits
    DROP CONSTRAINT IF EXISTS deposits_session_id_fkey;

ALTER TABLE deposits
    ADD CONSTRAINT deposits_session_id_fkey
        FOREIGN KEY (session_id)
        REFERENCES sessions(id)
        ON DELETE SET NULL;

ALTER TABLE withdrawals
    DROP CONSTRAINT IF EXISTS withdrawals_session_id_fkey;

ALTER TABLE withdrawals
    ADD CONSTRAINT withdrawals_session_id_fkey
        FOREIGN KEY (session_id)
        REFERENCES sessions(id)
        ON DELETE SET NULL;
