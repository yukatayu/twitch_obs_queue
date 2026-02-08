use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    #[serde(default)]
    pub server: ServerConfig,
    #[serde(default)]
    pub twitch: TwitchConfig,
    #[serde(default)]
    pub queue: QueueConfig,
}

impl Config {
    pub fn load(path: &str) -> anyhow::Result<Self> {
        let bytes = std::fs::read(path)?;
        let s = std::str::from_utf8(&bytes)?;
        let cfg: Config = toml::from_str(s)?;
        Ok(cfg)
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct ServerConfig {
    #[serde(default = "default_bind")]
    pub bind: String,
    #[serde(default = "default_static_dir")]
    pub static_dir: String,
    #[serde(default = "default_db_path")]
    pub db_path: String,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            bind: default_bind(),
            static_dir: default_static_dir(),
            db_path: default_db_path(),
        }
    }
}

fn default_bind() -> String {
    "127.0.0.1:3000".to_string()
}

fn default_static_dir() -> String {
    "static".to_string()
}

fn default_db_path() -> String {
    "data/app.db".to_string()
}

#[derive(Debug, Clone, Deserialize)]
pub struct TwitchConfig {
    #[serde(default)]
    pub client_id: String,
    #[serde(default)]
    pub client_secret: String,
    #[serde(default = "default_redirect_url")]
    pub redirect_url: String,

    /// If empty, we subscribe to all rewards but we won't enqueue.
    #[serde(default)]
    pub target_reward_id: String,

    /// If empty, cancel reward handling is disabled.
    #[serde(default)]
    pub cancel_reward_id: String,

    /// Cache TTL for user profiles (profile image URL) in seconds.
    /// Set 0 to always fetch from Helix.
    #[serde(default = "default_user_cache_ttl_secs")]
    pub user_cache_ttl_secs: u64,
}

impl Default for TwitchConfig {
    fn default() -> Self {
        Self {
            client_id: String::new(),
            client_secret: String::new(),
            redirect_url: default_redirect_url(),
            target_reward_id: String::new(),
            cancel_reward_id: String::new(),
            user_cache_ttl_secs: default_user_cache_ttl_secs(),
        }
    }
}

fn default_redirect_url() -> String {
    "http://localhost:3000/auth/callback".to_string()
}

fn default_user_cache_ttl_secs() -> u64 {
    24 * 60 * 60
}

#[derive(Debug, Clone, Deserialize)]
pub struct QueueConfig {
    #[serde(default = "default_participation_window_secs")]
    pub participation_window_secs: u64,

    #[serde(default = "default_processed_message_ttl_secs")]
    pub processed_message_ttl_secs: u64,
}

impl Default for QueueConfig {
    fn default() -> Self {
        Self {
            participation_window_secs: default_participation_window_secs(),
            processed_message_ttl_secs: default_processed_message_ttl_secs(),
        }
    }
}

fn default_participation_window_secs() -> u64 {
    24 * 60 * 60
}

fn default_processed_message_ttl_secs() -> u64 {
    24 * 60 * 60
}
