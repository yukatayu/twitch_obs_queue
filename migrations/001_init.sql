-- Queue items currently waiting
CREATE TABLE IF NOT EXISTS queue_items (
  id TEXT PRIMARY KEY,
  user_id TEXT NOT NULL UNIQUE,
  user_login TEXT NOT NULL,
  display_name TEXT NOT NULL,
  profile_image_url TEXT NOT NULL,
  enqueued_at INTEGER NOT NULL,
  position INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_queue_items_position ON queue_items(position);

-- Completed participations (used for fairness)
CREATE TABLE IF NOT EXISTS participations (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  user_id TEXT NOT NULL,
  completed_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_participations_user_time ON participations(user_id, completed_at);

-- OAuth tokens (single row)
CREATE TABLE IF NOT EXISTS oauth_tokens (
  id INTEGER PRIMARY KEY CHECK (id = 1),
  access_token TEXT NOT NULL,
  refresh_token TEXT NOT NULL,
  expires_at INTEGER NOT NULL
);

-- Simple KV store (broadcaster_id, broadcaster_login, etc.)
CREATE TABLE IF NOT EXISTS app_kv (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL
);

-- Dedup EventSub messages across restarts
CREATE TABLE IF NOT EXISTS processed_messages (
  message_id TEXT PRIMARY KEY,
  received_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_processed_messages_time ON processed_messages(received_at);
