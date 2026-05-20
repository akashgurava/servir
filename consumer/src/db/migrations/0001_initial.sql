-- Consumer-owned user profiles, keyed by auth user ID.
-- Created lazily on first authenticated access to GET /user.
--
-- Auth identity (username + password) lives in the auth database (auth.db).
-- This table holds app-specific profile data that can diverge from auth over time.
CREATE TABLE IF NOT EXISTS user_profiles (
    id         TEXT    NOT NULL PRIMARY KEY,  -- auth user_id (JWT sub)
    username   TEXT    NOT NULL,              -- copied from auth at first access
    created_at INTEGER NOT NULL
);
