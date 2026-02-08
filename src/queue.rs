use serde::{Deserialize, Serialize};
use sqlx::{FromRow, SqlitePool};
use uuid::Uuid;

use crate::util;

#[derive(Debug, Clone)]
pub struct NewQueueUser {
    pub user_id: String,
    pub user_login: String,
    pub display_name: String,
    pub profile_image_url: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct QueueItemDto {
    pub id: String,
    pub user_id: String,
    pub user_login: String,
    pub display_name: String,
    pub profile_image_url: String,
    pub enqueued_at: i64,
    pub position: i64,
    pub recent_participation_count: i64,
}

#[derive(Debug, Clone, Serialize)]
pub enum EnqueueOutcome {
    Added { id: String, position: i64 },
    AlreadyQueued,
}

#[derive(Debug, Copy, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeleteMode {
    Completed,
    Canceled,
}

#[derive(Debug, FromRow, Clone)]
struct QueueItemRow {
    id: String,
    user_id: String,
    user_login: String,
    display_name: String,
    profile_image_url: String,
    enqueued_at: i64,
    position: i64,
}

pub async fn list_queue(
    pool: &SqlitePool,
    participation_window_secs: i64,
) -> anyhow::Result<Vec<QueueItemDto>> {
    let now = util::now_epoch();
    let window_start = now - participation_window_secs;

    let rows = sqlx::query_as::<_, QueueItemRow>(
        r#"SELECT id, user_id, user_login, display_name, profile_image_url, enqueued_at, position
           FROM queue_items
           ORDER BY position ASC"#,
    )
    .fetch_all(pool)
    .await?;

    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        let c = count_participations(pool, &r.user_id, window_start).await?;
        out.push(QueueItemDto {
            id: r.id,
            user_id: r.user_id,
            user_login: r.user_login,
            display_name: r.display_name,
            profile_image_url: r.profile_image_url,
            enqueued_at: r.enqueued_at,
            position: r.position,
            recent_participation_count: c,
        });
    }

    Ok(out)
}

pub async fn is_user_queued(pool: &SqlitePool, user_id: &str) -> anyhow::Result<bool> {
    let row = sqlx::query("SELECT 1 FROM queue_items WHERE user_id = ?1 LIMIT 1")
        .bind(user_id)
        .fetch_optional(pool)
        .await?;
    Ok(row.is_some())
}

pub async fn cancel_by_user_id(pool: &SqlitePool, user_id: &str) -> anyhow::Result<bool> {
    let id = sqlx::query_scalar::<_, String>(
        r#"SELECT id
           FROM queue_items
           WHERE user_id = ?1
           LIMIT 1"#,
    )
    .bind(user_id)
    .fetch_optional(pool)
    .await?;

    let Some(id) = id else {
        return Ok(false);
    };

    delete_item(pool, &id, DeleteMode::Canceled).await?;
    Ok(true)
}

pub async fn enqueue_user(
    pool: &SqlitePool,
    participation_window_secs: i64,
    user: NewQueueUser,
) -> anyhow::Result<EnqueueOutcome> {
    let now = util::now_epoch();
    let window_start = now - participation_window_secs;

    let mut tx = pool.begin().await?;

    // Already queued?
    let existing = sqlx::query_as::<_, QueueItemRow>(
        r#"SELECT id, user_id, user_login, display_name, profile_image_url, enqueued_at, position
           FROM queue_items
           WHERE user_id = ?1
           LIMIT 1"#,
    )
    .bind(&user.user_id)
    .fetch_optional(&mut *tx)
    .await?;

    if existing.is_some() {
        tx.rollback().await?;
        return Ok(EnqueueOutcome::AlreadyQueued);
    }

    // Fetch current queue in order
    let current = sqlx::query_as::<_, QueueItemRow>(
        r#"SELECT id, user_id, user_login, display_name, profile_image_url, enqueued_at, position
           FROM queue_items
           ORDER BY position ASC"#,
    )
    .fetch_all(&mut *tx)
    .await?;

    let my_count = count_participations_tx(&mut tx, &user.user_id, window_start).await?;

    // Decide insertion point: before the first user who has strictly MORE completed participations
    let mut insert_pos: i64 = current.len() as i64;
    for (idx, item) in current.iter().enumerate() {
        let c = count_participations_tx(&mut tx, &item.user_id, window_start).await?;
        if c > my_count {
            insert_pos = idx as i64;
            break;
        }
    }

    // Shift down items at/after insert_pos
    sqlx::query(
        r#"UPDATE queue_items
           SET position = position + 1
           WHERE position >= ?1"#,
    )
    .bind(insert_pos)
    .execute(&mut *tx)
    .await?;

    let id = Uuid::new_v4().to_string();
    sqlx::query(
        r#"INSERT INTO queue_items (id, user_id, user_login, display_name, profile_image_url, enqueued_at, position)
           VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)"#,
    )
    .bind(&id)
    .bind(&user.user_id)
    .bind(&user.user_login)
    .bind(&user.display_name)
    .bind(&user.profile_image_url)
    .bind(now)
    .bind(insert_pos)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;

    Ok(EnqueueOutcome::Added {
        id,
        position: insert_pos,
    })
}

pub async fn delete_item(
    pool: &SqlitePool,
    id: &str,
    mode: DeleteMode,
) -> anyhow::Result<()> {
    let now = util::now_epoch();
    let mut tx = pool.begin().await?;

    // Find item
    let item = sqlx::query_as::<_, QueueItemRow>(
        r#"SELECT id, user_id, user_login, display_name, profile_image_url, enqueued_at, position
           FROM queue_items
           WHERE id = ?1"#,
    )
    .bind(id)
    .fetch_optional(&mut *tx)
    .await?;

    let Some(item) = item else {
        tx.rollback().await?;
        anyhow::bail!("queue item not found");
    };

    // Remove
    sqlx::query("DELETE FROM queue_items WHERE id = ?1")
        .bind(id)
        .execute(&mut *tx)
        .await?;

    // Close gap
    sqlx::query(
        r#"UPDATE queue_items
           SET position = position - 1
           WHERE position > ?1"#,
    )
    .bind(item.position)
    .execute(&mut *tx)
    .await?;

    // If completed, add a participation record (used for fairness)
    if matches!(mode, DeleteMode::Completed) {
        sqlx::query(
            r#"INSERT INTO participations (user_id, completed_at)
               VALUES (?1, ?2)"#,
        )
        .bind(&item.user_id)
        .bind(now)
        .execute(&mut *tx)
        .await?;
    }

    tx.commit().await?;
    Ok(())
}

pub async fn move_up(pool: &SqlitePool, id: &str) -> anyhow::Result<()> {
    move_by(pool, id, -1).await
}

pub async fn move_down(pool: &SqlitePool, id: &str) -> anyhow::Result<()> {
    move_by(pool, id, 1).await
}

async fn move_by(pool: &SqlitePool, id: &str, delta: i64) -> anyhow::Result<()> {
    let mut tx = pool.begin().await?;

    let item = sqlx::query_as::<_, QueueItemRow>(
        r#"SELECT id, user_id, user_login, display_name, profile_image_url, enqueued_at, position
           FROM queue_items
           WHERE id = ?1"#,
    )
    .bind(id)
    .fetch_optional(&mut *tx)
    .await?;

    let Some(item) = item else {
        tx.rollback().await?;
        anyhow::bail!("queue item not found");
    };

    let new_pos = item.position + delta;
    if new_pos < 0 {
        tx.rollback().await?;
        return Ok(());
    }

    let swap = sqlx::query_as::<_, QueueItemRow>(
        r#"SELECT id, user_id, user_login, display_name, profile_image_url, enqueued_at, position
           FROM queue_items
           WHERE position = ?1
           LIMIT 1"#,
    )
    .bind(new_pos)
    .fetch_optional(&mut *tx)
    .await?;

    let Some(swap) = swap else {
        tx.rollback().await?;
        return Ok(());
    };

    // Swap positions
    sqlx::query("UPDATE queue_items SET position = ?1 WHERE id = ?2")
        .bind(new_pos)
        .bind(&item.id)
        .execute(&mut *tx)
        .await?;

    sqlx::query("UPDATE queue_items SET position = ?1 WHERE id = ?2")
        .bind(item.position)
        .bind(&swap.id)
        .execute(&mut *tx)
        .await?;

    tx.commit().await?;
    Ok(())
}

async fn count_participations(pool: &SqlitePool, user_id: &str, window_start: i64) -> anyhow::Result<i64> {
    let row = sqlx::query_as::<_, CountRow>(
        r#"SELECT COUNT(*) as c
           FROM participations
           WHERE user_id = ?1 AND completed_at >= ?2"#,
    )
    .bind(user_id)
    .bind(window_start)
    .fetch_one(pool)
    .await?;
    Ok(row.c)
}

async fn count_participations_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    user_id: &str,
    window_start: i64,
) -> anyhow::Result<i64> {
    let row = sqlx::query_as::<_, CountRow>(
        r#"SELECT COUNT(*) as c
           FROM participations
           WHERE user_id = ?1 AND completed_at >= ?2"#,
    )
    .bind(user_id)
    .bind(window_start)
    .fetch_one(&mut **tx)
    .await?;
    Ok(row.c)
}

#[derive(Debug, FromRow)]
struct CountRow {
    c: i64,
}
