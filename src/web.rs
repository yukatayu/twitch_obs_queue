use std::sync::Arc;

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Redirect},
    routing::{get, post},
    Json, Router,
};
use axum::routing::get_service;
use serde::{Deserialize, Serialize};
use tower_http::services::{ServeDir, ServeFile};
use tracing::{error, info};

use crate::{db, queue, twitch, util, AppState};

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("bad request: {0}")]
    BadRequest(String),
    #[error("unauthorized: {0}")]
    Unauthorized(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error(transparent)]
    Internal(#[from] anyhow::Error),
}

impl IntoResponse for ApiError {
    fn into_response(self) -> axum::response::Response {
        let (status, msg) = match &self {
            ApiError::BadRequest(s) => (StatusCode::BAD_REQUEST, s.clone()),
            ApiError::Unauthorized(s) => (StatusCode::UNAUTHORIZED, s.clone()),
            ApiError::NotFound(s) => (StatusCode::NOT_FOUND, s.clone()),
            ApiError::Internal(e) => {
                error!(error=?e, "internal error");
                (StatusCode::INTERNAL_SERVER_ERROR, "internal error".to_string())
            }
        };
        (status, msg).into_response()
    }
}

type ApiResult<T> = Result<T, ApiError>;

pub fn router(state: Arc<AppState>) -> Router {
    let static_dir = state.config.server.static_dir.clone();
    let obs_file = format!("{static_dir}/obs.html");
    let admin_file = format!("{static_dir}/admin.html");
    let rewards_file = format!("{static_dir}/rewards.html");
    let css_creator_file = format!("{static_dir}/css_creator.html");
    let assets_dir = format!("{static_dir}/assets");

    Router::new()
        .route("/", get(|| async { Redirect::temporary("/admin") }))
        .route("/obs", get_service(ServeFile::new(obs_file)))
        .route("/admin", get_service(ServeFile::new(admin_file)))
        .route("/admin/rewards", get_service(ServeFile::new(rewards_file)))
        .route("/admin/css", get_service(ServeFile::new(css_creator_file)))
        .nest_service("/assets", ServeDir::new(assets_dir))
        // Auth
        .route("/auth/start", get(auth_start))
        .route("/auth/callback", get(auth_callback))
        .route("/auth/logout", post(auth_logout))
        // API
        .route("/api/status", get(api_status))
        .route("/api/queue", get(api_queue))
        .route("/api/queue/:id/delete", post(api_queue_delete))
        .route("/api/queue/:id/move_up", post(api_queue_move_up))
        .route("/api/queue/:id/move_down", post(api_queue_move_down))
        .route("/api/rewards", get(api_rewards))
        .with_state(state)
}

#[derive(Debug, Deserialize)]
struct AuthCallbackQuery {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
}

async fn auth_start(State(app): State<Arc<AppState>>) -> ApiResult<Redirect> {
    if util::is_blank(&app.config.twitch.client_id) || util::is_blank(&app.config.twitch.client_secret)
    {
        return Err(ApiError::BadRequest(
            "config.toml の twitch.client_id / twitch.client_secret を設定してください".to_string(),
        ));
    }

    let state = uuid::Uuid::new_v4().to_string();
    {
        let mut w = app.oauth_state.write().await;
        *w = Some(state.clone());
    }

    let url = twitch::build_authorize_url(&app.config, &state)?;
    Ok(Redirect::temporary(&url))
}

async fn auth_callback(
    State(app): State<Arc<AppState>>,
    Query(q): Query<AuthCallbackQuery>,
) -> ApiResult<Redirect> {
    if let Some(err) = q.error {
        let desc = q.error_description.unwrap_or_default();
        return Err(ApiError::BadRequest(format!("oauth error: {err} {desc}")));
    }

    let code = q
        .code
        .ok_or_else(|| ApiError::BadRequest("missing code".to_string()))?;
    let returned_state = q
        .state
        .ok_or_else(|| ApiError::BadRequest("missing state".to_string()))?;

    let expected_state = { app.oauth_state.read().await.clone() };
    if expected_state.as_deref() != Some(returned_state.as_str()) {
        return Err(ApiError::BadRequest("state mismatch".to_string()));
    }

    let token = twitch::exchange_code_for_token(app.as_ref(), &code).await?;
    db::upsert_oauth_token(&app.db, &token).await?;

    // Resolve & store broadcaster info
    match twitch::helix_get_self(app.as_ref(), &token.access_token).await {
        Ok(me) => {
            db::set_broadcaster_id(&app.db, &me.id).await?;
            db::set_broadcaster_login(&app.db, &me.login).await?;
            info!(broadcaster_id=%me.id, broadcaster_login=%me.login, "authorized");
        }
        Err(e) => {
            error!(error=?e, "authorized but failed to resolve broadcaster via helix");
        }
    }

    {
        let mut w = app.oauth_state.write().await;
        *w = None;
    }

    Ok(Redirect::temporary("/admin"))
}

async fn auth_logout(State(app): State<Arc<AppState>>) -> ApiResult<StatusCode> {
    db::delete_oauth_token(&app.db).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Serialize)]
struct StatusDto {
    authenticated: bool,
    broadcaster_id: Option<String>,
    broadcaster_login: Option<String>,
    target_reward_ids: Vec<String>,
    participation_window_secs: u64,
    server_time: i64,
}

async fn api_status(State(app): State<Arc<AppState>>) -> ApiResult<Json<StatusDto>> {
    let authenticated = db::has_validish_token(&app.db).await?;
    let broadcaster_id = db::get_broadcaster_id(&app.db).await?;
    let broadcaster_login = db::get_broadcaster_login(&app.db).await?;

    Ok(Json(StatusDto {
        authenticated,
        broadcaster_id,
        broadcaster_login,
        target_reward_ids: app.config.twitch.target_reward_ids.clone(),
        participation_window_secs: app.config.queue.participation_window_secs,
        server_time: util::now_epoch(),
    }))
}

async fn api_queue(State(app): State<Arc<AppState>>) -> ApiResult<Json<Vec<queue::QueueItemDto>>> {
    let win = app.config.queue.participation_window_secs as i64;
    let q = queue::list_queue(&app.db, win).await?;
    Ok(Json(q))
}

#[derive(Debug, Deserialize)]
struct DeleteBody {
    mode: queue::DeleteMode,
}

async fn api_queue_delete(
    State(app): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<DeleteBody>,
) -> ApiResult<StatusCode> {
    queue::delete_item(&app.db, &id, body.mode)
        .await
        .map_err(|e| {
            if e.to_string().contains("not found") {
                ApiError::NotFound("queue item not found".to_string())
            } else {
                ApiError::Internal(e)
            }
        })?;
    Ok(StatusCode::NO_CONTENT)
}

async fn api_queue_move_up(
    State(app): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    queue::move_up(&app.db, &id).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn api_queue_move_down(
    State(app): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    queue::move_down(&app.db, &id).await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn get_valid_access_token(app: &Arc<AppState>) -> ApiResult<String> {
    let Some(mut t) = db::get_oauth_token(&app.db).await? else {
        return Err(ApiError::Unauthorized("not authenticated".to_string()));
    };

    if t.expires_at <= util::now_epoch() + 60 {
        let new_t = twitch::refresh_access_token(app.as_ref(), &t.refresh_token).await?;
        db::upsert_oauth_token(&app.db, &new_t).await?;
        t = new_t;
    }

    Ok(t.access_token)
}

async fn api_rewards(State(app): State<Arc<AppState>>) -> ApiResult<Json<Vec<twitch::HelixReward>>> {
    let access_token = get_valid_access_token(&app).await?;

    let broadcaster_id = match db::get_broadcaster_id(&app.db).await? {
        Some(id) => id,
        None => {
            let me = twitch::helix_get_self(app.as_ref(), &access_token).await?;
            db::set_broadcaster_id(&app.db, &me.id).await?;
            db::set_broadcaster_login(&app.db, &me.login).await?;
            me.id
        }
    };

    let rewards = twitch::helix_get_custom_rewards(app.as_ref(), &access_token, &broadcaster_id).await?;
    Ok(Json(rewards))
}
