-- Cache of Twitch user profiles (mainly profile image url)
CREATE TABLE IF NOT EXISTS user_cache (
  user_id TEXT PRIMARY KEY,
  user_login TEXT NOT NULL,
  display_name TEXT NOT NULL,
  profile_image_url TEXT NOT NULL,
  updated_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_user_cache_updated_at ON user_cache(updated_at);
