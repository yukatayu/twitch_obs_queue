mod config;
mod db;
mod queue;
mod twitch;
mod util;
mod web;

use std::sync::Arc;

use anyhow::Context;
use config::Config;
use sqlx::SqlitePool;
use tokio::sync::RwLock;
use tracing::{error, info};

pub struct AppState {
    pub config: Arc<Config>,
    pub db: SqlitePool,
    pub http: reqwest::Client,
    /// OAuth state (CSRF) for the current login attempt.
    pub oauth_state: RwLock<Option<String>>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,twitch_obs_queue=info".into()),
        )
        .init();

    let config_path = std::env::var("CONFIG").unwrap_or_else(|_| "config.toml".to_string());
    let config = Config::load(&config_path).with_context(|| format!("failed to load {config_path}"))?;

    let db = db::init_pool(&config.server.db_path)
        .await
        .with_context(|| format!("failed to init sqlite at {}", config.server.db_path))?;

    let http = reqwest::Client::builder()
        .user_agent("twitch-obs-queue/0.1")
        .build()?;

    let state = Arc::new(AppState {
        config: Arc::new(config),
        db,
        http,
        oauth_state: RwLock::new(None),
    });

    // Background: EventSub websocket + enqueue logic
    {
        let state = Arc::clone(&state);
        tokio::spawn(async move {
            if let Err(e) = twitch::run_eventsub_loop(state).await {
                error!(error = ?e, "eventsub loop exited");
            }
        });
    }

    // Background: cleanup processed message ids
    {
        let state = Arc::clone(&state);
        tokio::spawn(async move {
            loop {
                let ttl = state.config.queue.processed_message_ttl_secs as i64;
                let cutoff = util::now_epoch() - ttl;
                match db::cleanup_processed_messages(&state.db, cutoff).await {
                    Ok(n) if n > 0 => info!(deleted = n, "cleaned processed_messages"),
                    Ok(_) => {}
                    Err(e) => error!(error = ?e, "failed to cleanup processed_messages"),
                }
                tokio::time::sleep(std::time::Duration::from_secs(60 * 10)).await;
            }
        });
    }

    let app = web::router(state.clone());

    let addr = state
        .config
        .server
        .bind
        .parse::<std::net::SocketAddr>()
        .context("server.bind must be like 127.0.0.1:3000")?;

    info!(%addr, "server starting");

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}
