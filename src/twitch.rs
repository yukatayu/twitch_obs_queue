use std::sync::Arc;

use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio_tungstenite::tungstenite::Message;
use tracing::{debug, error, info, warn};
use url::Url;

use crate::{db, queue, util, AppState};

const AUTHORIZE_ENDPOINT: &str = "https://id.twitch.tv/oauth2/authorize";
const TOKEN_ENDPOINT: &str = "https://id.twitch.tv/oauth2/token";
const HELIX_ENDPOINT: &str = "https://api.twitch.tv/helix";
const EVENTSUB_WS_URL: &str = "wss://eventsub.wss.twitch.tv/ws";

const REQUIRED_SCOPES: &str = "channel:read:redemptions";

const SUB_TYPE_REDEMPTION_ADD: &str = "channel.channel_points_custom_reward_redemption.add";

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: String,
    expires_in: i64,
    token_type: String,
    #[serde(default)]
    scope: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct HelixResponse<T> {
    data: Vec<T>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct HelixUser {
    pub id: String,
    pub login: String,
    display_name: String,
    profile_image_url: String,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct HelixReward {
    pub id: String,
    pub title: String,
    pub cost: i64,
    pub is_enabled: bool,
}

#[derive(Debug, Deserialize)]
struct HelixRewardsResponse {
    data: Vec<HelixReward>,
}

pub fn build_authorize_url(config: &crate::config::Config, state: &str) -> anyhow::Result<String> {
    let mut url = Url::parse(AUTHORIZE_ENDPOINT)?;
    url.query_pairs_mut()
        .append_pair("client_id", &config.twitch.client_id)
        .append_pair("redirect_uri", &config.twitch.redirect_url)
        .append_pair("response_type", "code")
        .append_pair("scope", REQUIRED_SCOPES)
        .append_pair("state", state);
    Ok(url.to_string())
}

pub async fn exchange_code_for_token(
    state: &AppState,
    code: &str,
) -> anyhow::Result<db::OAuthToken> {
    let params = [
        ("client_id", state.config.twitch.client_id.as_str()),
        ("client_secret", state.config.twitch.client_secret.as_str()),
        ("code", code),
        ("grant_type", "authorization_code"),
        ("redirect_uri", state.config.twitch.redirect_url.as_str()),
    ];

    let resp = state
        .http
        .post(TOKEN_ENDPOINT)
        .form(&params)
        .send()
        .await?
        .error_for_status()?;

    let token: TokenResponse = resp.json().await?;
    Ok(db::OAuthToken {
        access_token: token.access_token,
        refresh_token: token.refresh_token,
        expires_at: util::now_epoch() + token.expires_in,
    })
}

pub async fn refresh_access_token(
    state: &AppState,
    refresh_token: &str,
) -> anyhow::Result<db::OAuthToken> {
    let params = [
        ("client_id", state.config.twitch.client_id.as_str()),
        ("client_secret", state.config.twitch.client_secret.as_str()),
        ("refresh_token", refresh_token),
        ("grant_type", "refresh_token"),
    ];

    let resp = state
        .http
        .post(TOKEN_ENDPOINT)
        .form(&params)
        .send()
        .await?
        .error_for_status()?;

    let token: TokenResponse = resp.json().await?;
    Ok(db::OAuthToken {
        access_token: token.access_token,
        refresh_token: token.refresh_token,
        expires_at: util::now_epoch() + token.expires_in,
    })
}

pub async fn helix_get_self(state: &AppState, access_token: &str) -> anyhow::Result<HelixUser> {
    let url = format!("{HELIX_ENDPOINT}/users");
    let resp = state
        .http
        .get(url)
        .header("Client-Id", &state.config.twitch.client_id)
        .header("Authorization", format!("Bearer {access_token}"))
        .send()
        .await?
        .error_for_status()?;

    let data: HelixResponse<HelixUser> = resp.json().await?;
    let user = data
        .data
        .into_iter()
        .next()
        .ok_or_else(|| anyhow::anyhow!("helix /users returned empty data"))?;
    Ok(user)
}

pub async fn helix_get_user_by_id(
    state: &AppState,
    access_token: &str,
    user_id: &str,
) -> anyhow::Result<HelixUser> {
    let mut url = Url::parse(&format!("{HELIX_ENDPOINT}/users"))?;
    url.query_pairs_mut().append_pair("id", user_id);
    let resp = state
        .http
        .get(url)
        .header("Client-Id", &state.config.twitch.client_id)
        .header("Authorization", format!("Bearer {access_token}"))
        .send()
        .await?
        .error_for_status()?;

    let data: HelixResponse<HelixUser> = resp.json().await?;
    let user = data
        .data
        .into_iter()
        .next()
        .ok_or_else(|| anyhow::anyhow!("helix /users returned empty data"))?;
    Ok(user)
}

async fn get_profile_image_url_cached(
    state: &AppState,
    access_token: &str,
    user_id: &str,
) -> anyhow::Result<String> {
    let now = util::now_epoch();
    let ttl = state.config.twitch.user_cache_ttl_secs as i64;

    // Grab cache first (also used as fallback if Helix fails)
    let cached = db::get_cached_user_profile(&state.db, user_id).await?;
    if ttl > 0 {
        if let Some(c) = &cached {
            if now.saturating_sub(c.updated_at) <= ttl {
                return Ok(c.profile_image_url.clone());
            }
        }
    }

    match helix_get_user_by_id(state, access_token, user_id).await {
        Ok(u) => {
            // Upsert cache
            let profile = db::CachedUserProfile {
                user_id: u.id,
                user_login: u.login,
                display_name: u.display_name,
                profile_image_url: u.profile_image_url.clone(),
                updated_at: now,
            };
            // Best-effort cache write (should not block enqueue)
            if let Err(e) = db::upsert_cached_user_profile(&state.db, &profile).await {
                warn!(error=?e, user_id=%user_id, "failed to upsert user cache");
            }
            Ok(profile.profile_image_url)
        }
        Err(e) => {
            if let Some(c) = cached {
                warn!(error=?e, user_id=%user_id, "helix user fetch failed; using cached profile_image_url");
                Ok(c.profile_image_url)
            } else {
                Err(e)
            }
        }
    }
}

pub async fn helix_get_custom_rewards(
    state: &AppState,
    access_token: &str,
    broadcaster_id: &str,
) -> anyhow::Result<Vec<HelixReward>> {
    let mut url = Url::parse(&format!("{HELIX_ENDPOINT}/channel_points/custom_rewards"))?;
    url.query_pairs_mut()
        .append_pair("broadcaster_id", broadcaster_id)
        .append_pair("only_manageable_rewards", "false");

    let resp = state
        .http
        .get(url)
        .header("Client-Id", &state.config.twitch.client_id)
        .header("Authorization", format!("Bearer {access_token}"))
        .send()
        .await?
        .error_for_status()?;

    let data: HelixRewardsResponse = resp.json().await?;
    Ok(data.data)
}

// --- EventSub subscription maintenance -------------------------------------

#[derive(Debug, Deserialize)]
struct HelixEventSubListResponse {
    #[serde(default)]
    data: Vec<HelixEventSubSubscription>,
    #[serde(default)]
    pagination: HelixPagination,
}

#[derive(Debug, Default, Deserialize)]
struct HelixPagination {
    #[serde(default)]
    cursor: Option<String>,
}

#[derive(Debug, Deserialize)]
struct HelixEventSubSubscription {
    id: String,
    status: String,
    #[serde(rename = "type")]
    typ: String,
    condition: serde_json::Value,
    transport: HelixEventSubTransport,
}

#[derive(Debug, Deserialize)]
struct HelixEventSubTransport {
    method: String,
    #[serde(default)]
    session_id: Option<String>,
    #[serde(default)]
    connected_at: Option<String>,
    #[serde(default)]
    disconnected_at: Option<String>,
}

async fn helix_list_eventsub_subscriptions_by_type(
    state: &AppState,
    access_token: &str,
    typ: &str,
) -> anyhow::Result<Vec<HelixEventSubSubscription>> {
    let mut out = Vec::new();
    let mut cursor: Option<String> = None;

    for _page in 0..50 {
        let mut url = Url::parse(&format!("{HELIX_ENDPOINT}/eventsub/subscriptions"))?;
        {
            let mut qp = url.query_pairs_mut();
            qp.append_pair("type", typ);
            if let Some(c) = &cursor {
                qp.append_pair("after", c);
            }
        }

        let resp = state
            .http
            .get(url)
            .header("Client-Id", &state.config.twitch.client_id)
            .header("Authorization", format!("Bearer {access_token}"))
            .send()
            .await?;

        // If we get rate-limited, just stop (best-effort)
        if resp.status().as_u16() == 429 {
            anyhow::bail!("rate limited while listing eventsub subscriptions");
        }

        let resp = resp.error_for_status()?;
        let body: HelixEventSubListResponse = resp.json().await?;

        out.extend(body.data);
        cursor = body.pagination.cursor;
        if cursor.is_none() {
            break;
        }
    }

    Ok(out)
}

async fn helix_delete_eventsub_subscription(
    state: &AppState,
    access_token: &str,
    id: &str,
) -> anyhow::Result<()> {
    let mut url = Url::parse(&format!("{HELIX_ENDPOINT}/eventsub/subscriptions"))?;
    url.query_pairs_mut().append_pair("id", id);

    let resp = state
        .http
        .delete(url)
        .header("Client-Id", &state.config.twitch.client_id)
        .header("Authorization", format!("Bearer {access_token}"))
        .send()
        .await?;

    if resp.status().as_u16() == 404 {
        // Already deleted
        return Ok(());
    }
    if resp.status().as_u16() == 429 {
        anyhow::bail!("rate limited while deleting eventsub subscription");
    }

    resp.error_for_status()?;
    Ok(())
}

async fn cleanup_stale_websocket_redemption_subscriptions(
    state: &AppState,
    access_token: &str,
    broadcaster_id: &str,
) -> anyhow::Result<u64> {
    let subs = helix_list_eventsub_subscriptions_by_type(state, access_token, SUB_TYPE_REDEMPTION_ADD).await?;
    let mut deleted = 0u64;

    for s in subs {
        if s.typ != SUB_TYPE_REDEMPTION_ADD {
            continue;
        }
        if s.transport.method != "websocket" {
            continue;
        }

        // Only touch subs for *our* broadcaster
        let cond_bid = s
            .condition
            .get("broadcaster_user_id")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if cond_bid != broadcaster_id {
            continue;
        }

        if s.status == "enabled" {
            continue;
        }

        debug!(sub_id=%s.id, status=%s.status, session_id=?s.transport.session_id, connected_at=?s.transport.connected_at, disconnected_at=?s.transport.disconnected_at, "deleting stale websocket subscription");
        helix_delete_eventsub_subscription(state, access_token, &s.id).await?;
        deleted += 1;
    }

    Ok(deleted)
}

#[derive(Debug, Deserialize)]
struct WsEnvelope {
    metadata: WsMetadata,
    payload: serde_json::Value,
}

#[derive(Debug, Deserialize)]
struct WsMetadata {
    message_id: String,
    message_type: String,
    #[serde(default)]
    subscription_type: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SessionWelcomePayload {
    session: SessionInfo,
}

#[derive(Debug, Deserialize)]
struct SessionInfo {
    id: String,
    #[serde(default)]
    reconnect_url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct NotificationPayload {
    subscription: serde_json::Value,
    event: RedemptionEvent,
}

#[derive(Debug, Deserialize)]
struct RedemptionEvent {
    user_id: String,
    user_login: String,
    user_name: String,
    reward: RewardInfo,
}

#[derive(Debug, Deserialize)]
struct RewardInfo {
    id: String,
    title: String,
    cost: i64,
}

#[derive(Debug, Serialize)]
struct CreateSubRequest<'a> {
    #[serde(rename = "type")]
    typ: &'a str,
    version: &'a str,
    condition: SubCondition<'a>,
    transport: SubTransport<'a>,
}

#[derive(Debug, Serialize)]
struct SubCondition<'a> {
    broadcaster_user_id: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    reward_id: Option<&'a str>,
}

#[derive(Debug, Serialize)]
struct SubTransport<'a> {
    method: &'a str,
    session_id: &'a str,
}

pub async fn run_eventsub_loop(state: Arc<AppState>) -> anyhow::Result<()> {
    if util::is_blank(&state.config.twitch.client_id) || util::is_blank(&state.config.twitch.client_secret) {
        warn!("twitch.client_id / twitch.client_secret are empty. Set them in config.toml.");
    }

    let mut ws_url = Url::parse(EVENTSUB_WS_URL)?;
    let mut need_subscribe = true;

    loop {
        // We cannot do anything without a token.
        let Some(mut token) = db::get_oauth_token(&state.db).await? else {
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            continue;
        };

        // Refresh if close to expiry
        if token.expires_at <= util::now_epoch() + 60 {
            match refresh_access_token(&state, &token.refresh_token).await {
                Ok(new_token) => {
                    db::upsert_oauth_token(&state.db, &new_token).await?;
                    token = new_token;
                    info!("refreshed twitch access token");
                }
                Err(e) => {
                    warn!(error = ?e, "failed to refresh token; need re-auth");
                    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                    continue;
                }
            }
        }

        // Ensure broadcaster id is known (derived from the authorized user)
        let broadcaster_id = match db::get_broadcaster_id(&state.db).await? {
            Some(id) => id,
            None => {
                match helix_get_self(&state, &token.access_token).await {
                    Ok(me) => {
                        db::set_broadcaster_id(&state.db, &me.id).await?;
                        db::set_broadcaster_login(&state.db, &me.login).await?;
                        info!(broadcaster_id = %me.id, broadcaster_login = %me.login, "resolved broadcaster");
                        me.id
                    }
                    Err(e) => {
                        warn!(error = ?e, "failed to resolve broadcaster; waiting");
                        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                        continue;
                    }
                }
            }
        };

        info!(ws = %ws_url, "connecting to EventSub WebSocket");
        let connect = tokio_tungstenite::connect_async(ws_url.as_str()).await;
        let (ws_stream, _resp) = match connect {
            Ok(x) => x,
            Err(e) => {
                warn!(error = ?e, "failed to connect websocket; retrying");
                tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                continue;
            }
        };

        let (mut write, mut read) = ws_stream.split();

        // If we receive a session_reconnect message, we should connect to the given URL.
        // In that case, subscriptions are migrated automatically and we must NOT recreate them.
        let mut received_reconnect = false;

        // Read loop
        while let Some(msg) = read.next().await {
            let msg = match msg {
                Ok(m) => m,
                Err(e) => {
                    warn!(error = ?e, "websocket read error");
                    break;
                }
            };

            match msg {
                Message::Text(text) => {
                    let env: WsEnvelope = match serde_json::from_str(&text) {
                        Ok(v) => v,
                        Err(e) => {
                            debug!(error = ?e, raw = %text, "failed to parse ws json");
                            continue;
                        }
                    };

                    match env.metadata.message_type.as_str() {
                        "session_welcome" => {
                            let payload: SessionWelcomePayload = serde_json::from_value(env.payload)?;
                            info!(session_id = %payload.session.id, "eventsub session welcome");

                            if need_subscribe {
                                if let Err(e) = create_redemption_subscription(
                                    &state,
                                    &token.access_token,
                                    &payload.session.id,
                                    &broadcaster_id,
                                )
                                .await
                                {
                                    warn!(error = ?e, "failed to create subscription");
                                } else {
                                    info!("created subscription(s)");
                                    need_subscribe = false;

                                    // Best-effort cleanup of stale/disconnected subscriptions.
                                    // Do this AFTER subscribing so we don't risk missing the 10s subscribe window.
                                    let state2 = Arc::clone(&state);
                                    let access_token2 = token.access_token.clone();
                                    let broadcaster_id2 = broadcaster_id.clone();
                                    tokio::spawn(async move {
                                        match cleanup_stale_websocket_redemption_subscriptions(
                                            state2.as_ref(),
                                            &access_token2,
                                            &broadcaster_id2,
                                        )
                                        .await
                                        {
                                            Ok(n) if n > 0 => info!(deleted = n, "cleaned stale EventSub subscriptions"),
                                            Ok(_) => {}
                                            Err(e) => warn!(error=?e, "failed to cleanup stale EventSub subscriptions"),
                                        }
                                    });
                                }
                            } else {
                                // On session_reconnect, subscriptions are migrated automatically.
                                info!("reconnected; keeping existing subscriptions");
                            }
                        }
                        "session_keepalive" => {
                            // nothing
                        }
                        "notification" => {
                            if env.metadata.subscription_type.as_deref() != Some(SUB_TYPE_REDEMPTION_ADD)
                            {
                                continue;
                            }

                            // Dedup (EventSub can resend a message_id)
                            let already = db::is_processed_message(&state.db, &env.metadata.message_id).await?;
                            if already {
                                debug!(message_id = %env.metadata.message_id, "duplicate notification ignored");
                                continue;
                            }
                            db::mark_processed_message(&state.db, &env.metadata.message_id, util::now_epoch()).await?;

                            let payload: NotificationPayload = match serde_json::from_value(env.payload) {
                                Ok(v) => v,
                                Err(e) => {
                                    warn!(error=?e, "failed to parse notification payload");
                                    continue;
                                }
                            };

                            // Optional extra safety check
                            if !util::is_blank(&state.config.twitch.target_reward_id)
                                && payload.event.reward.id != state.config.twitch.target_reward_id
                            {
                                debug!(reward_id=%payload.event.reward.id, title=%payload.event.reward.title, "non-target reward ignored");
                                continue;
                            }

                            if util::is_blank(&state.config.twitch.target_reward_id) {
                                info!(
                                    reward_id = %payload.event.reward.id,
                                    reward_title = %payload.event.reward.title,
                                    user = %payload.event.user_name,
                                    "received redemption (target_reward_id not set; not enqueuing)"
                                );
                                continue;
                            }

                            // If already queued, ignore without hitting Helix.
                            if queue::is_user_queued(&state.db, &payload.event.user_id).await? {
                                info!(user_id=%payload.event.user_id, "already queued; ignoring redemption");
                                continue;
                            }

                            // Get profile image (cached)
                            let profile_image_url = match get_profile_image_url_cached(
                                &state,
                                &token.access_token,
                                &payload.event.user_id,
                            )
                            .await
                            {
                                Ok(url) => url,
                                Err(e) => {
                                    warn!(error=?e, user_id=%payload.event.user_id, "failed to resolve user profile_image_url");
                                    continue;
                                }
                            };

                            let new_user = queue::NewQueueUser {
                                user_id: payload.event.user_id,
                                user_login: payload.event.user_login,
                                display_name: payload.event.user_name,
                                profile_image_url,
                            };

                            let win = state.config.queue.participation_window_secs as i64;
                            match queue::enqueue_user(&state.db, win, new_user).await {
                                Ok(queue::EnqueueOutcome::AlreadyQueued) => {
                                    info!("already queued; ignoring redemption");
                                }
                                Ok(queue::EnqueueOutcome::Added { id, position }) => {
                                    info!(queue_id=%id, position, "enqueued user");
                                }
                                Err(e) => {
                                    error!(error=?e, "failed to enqueue");
                                }
                            }
                        }
                        "session_reconnect" => {
                            // reconnect_url includes existing subscriptions
                            let payload: SessionWelcomePayload = serde_json::from_value(env.payload)?;
                            let Some(url) = payload.session.reconnect_url else {
                                warn!("session_reconnect without reconnect_url");
                                break;
                            };
                            info!(reconnect_url=%url, "received session_reconnect");
                            ws_url = Url::parse(&url)?;
                            // keep need_subscribe=false (subs are migrated)
                            received_reconnect = true;
                            break;
                        }
                        "revocation" => {
                            warn!("subscription revoked (token revoked or user no longer exists). Re-auth required.");
                            // Force resubscribe after re-auth
                            need_subscribe = true;
                        }
                        other => {
                            debug!(message_type=%other, "unhandled ws message");
                        }
                    }
                }
                Message::Ping(payload) => {
                    // Best-effort Pong
                    let _ = write.send(Message::Pong(payload)).await;
                }
                Message::Close(frame) => {
                    info!(?frame, "websocket closed");
                    break;
                }
                _ => {}
            }
        }

        // If we drop out of the read loop without receiving session_reconnect,
        // it's a disconnect -> subscriptions need to be recreated in a NEW session.
        // (If we *did* receive session_reconnect, Twitch migrates subscriptions automatically.)
        if !received_reconnect {
            need_subscribe = true;
            ws_url = Url::parse(EVENTSUB_WS_URL)?;
        }
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    }
}

async fn create_redemption_subscription(
    state: &AppState,
    access_token: &str,
    session_id: &str,
    broadcaster_id: &str,
) -> anyhow::Result<()> {
    let reward_id_opt = if util::is_blank(&state.config.twitch.target_reward_id) {
        None
    } else {
        Some(state.config.twitch.target_reward_id.as_str())
    };

    let req = CreateSubRequest {
        typ: SUB_TYPE_REDEMPTION_ADD,
        version: "1",
        condition: SubCondition {
            broadcaster_user_id: broadcaster_id,
            reward_id: reward_id_opt,
        },
        transport: SubTransport {
            method: "websocket",
            session_id,
        },
    };

    let url = format!("{HELIX_ENDPOINT}/eventsub/subscriptions");
    let resp = state
        .http
        .post(url)
        .header("Client-Id", &state.config.twitch.client_id)
        .header("Authorization", format!("Bearer {access_token}"))
        .json(&req)
        .send()
        .await?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!("create subscription failed: {status} {body}");
    }

    Ok(())
}
