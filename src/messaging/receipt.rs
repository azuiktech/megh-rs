//! Receipts: which messages a participant has seen, and who is typing. Typing is published, never stored.

use axum::Extension;
use axum::Json;
use axum::http::StatusCode;
use axum::routing::post;
use serde::{Deserialize, Serialize};
use serde_json::json;
use utoipa::{OpenApi, ToSchema};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use uuid::Uuid;
use validator::Validate;

use super::error::MessagingError;
use super::messaging::{Messaging, thread_topic};
use super::participant;
use crate::auth::AccessToken;
use crate::auth::rest::{AppContext, Input, Key};
use crate::event::Event;

/// A thread with messages the participant has not seen.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, sqlx::FromRow, ToSchema)]
pub struct Unread {
    pub thread_id: Uuid,
    pub count: i64,
}

impl Messaging {
    /// Marks the thread read up to the message `up_to`; answers how many messages of others are left unread.
    pub async fn seen(&self, tenant_id: Uuid, thread_id: Uuid, participant: Uuid, up_to: Uuid) -> Result<u32, MessagingError> {
        let left: i32 = sqlx::query_scalar(
            "WITH moved AS (
                 UPDATE participants SET last_read_message_id = $4, updated_at = now()
                 WHERE tenant_id = $1 AND thread_id = $2 AND id = $3 AND deleted_at IS NULL
                   AND EXISTS (SELECT 1 FROM messages WHERE tenant_id = $1 AND thread_id = $2 AND id = $4 AND deleted_at IS NULL)
                 RETURNING id)
             SELECT (SELECT count(*) FROM messages WHERE tenant_id = $1 AND thread_id = $2 AND deleted_at IS NULL AND sender_id <> $3 AND id > $4)::int
             FROM moved",
        )
        .bind(tenant_id)
        .bind(thread_id)
        .bind(participant)
        .bind(up_to)
        .fetch_optional(&self.pool)
        .await?
        .ok_or(MessagingError::NotFound)?;
        self.announce(Event::new(thread_topic(tenant_id, thread_id), json!({"kind": "seen", "participant_id": participant, "up_to": up_to}))).await;
        Ok(left.unsigned_abs())
    }

    /// Publishes that `participant` is typing; nothing is stored.
    pub async fn typing(&self, tenant_id: Uuid, thread_id: Uuid, participant: Uuid) -> Result<(), MessagingError> {
        participant::require(&self.pool, tenant_id, thread_id, participant).await?;
        self.announce(Event::new(thread_topic(tenant_id, thread_id), json!({"kind": "typing", "participant_id": participant}))).await;
        Ok(())
    }

    /// The threads where `participant` (a participant, or the member or client behind one) has unread messages of others.
    pub async fn unread(&self, tenant_id: Uuid, participant: Uuid) -> Result<Vec<Unread>, MessagingError> {
        Ok(sqlx::query_as(
            "SELECT p.thread_id, count(m.id) AS count FROM participants p
             JOIN messages m ON m.tenant_id = p.tenant_id AND m.thread_id = p.thread_id AND m.deleted_at IS NULL
                            AND m.sender_id <> p.id AND (p.last_read_message_id IS NULL OR m.id > p.last_read_message_id)
             WHERE p.tenant_id = $1 AND (p.id = $2 OR p.member_id = $2 OR p.client_id = $2) AND p.deleted_at IS NULL
             GROUP BY p.thread_id",
        )
        .bind(tenant_id)
        .bind(participant)
        .fetch_all(&self.pool)
        .await?)
    }
}

#[derive(OpenApi)]
#[openapi(paths(seen))]
struct Actions;

pub fn routes() -> OpenApiRouter<AppContext> {
    OpenApiRouter::with_openapi(Actions::openapi())
        .routes(routes!(typing))
        .route("/tenants/{tenant_id}/threads/{id}/{verb}", post(seen))
}

/// The path segment of the custom method.
#[derive(Debug, Deserialize)]
enum Verb {
    #[serde(rename = ":seen")]
    Seen,
}

#[derive(Debug, Deserialize, Validate)]
struct Seen {
    #[serde(flatten)]
    key: Key,
    #[serde(rename = "verb")]
    _verb: Verb,
    up_to: Uuid,
}

#[derive(Debug, Serialize, ToSchema)]
struct Remaining {
    unread: u32,
}

/// The caller has read the thread up to the message `up_to`.
#[utoipa::path(
    post,
    path = "/tenants/{tenant_id}/threads/{id}:seen",
    tag = "threads",
    params(("tenant_id" = Uuid, Path), ("id" = Uuid, Path)),
    request_body(content = Object, description = "`{\"up_to\": message id}`"),
    responses((status = 200, body = Remaining), (status = 404)),
)]
async fn seen(
    Extension(messaging): Extension<Messaging>,
    Extension(token): Extension<AccessToken>,
    Input(seen): Input<Seen>,
) -> Result<Json<Remaining>, MessagingError> {
    let speaker = messaging.speaker(seen.key.tenant_id, seen.key.id, token.sub).await?;
    let unread = messaging.seen(seen.key.tenant_id, seen.key.id, speaker.id, seen.up_to).await?;
    Ok(Json(Remaining { unread }))
}

/// The caller is typing.
#[utoipa::path(
    post,
    path = "/tenants/{tenant_id}/threads/{id}/typing",
    tag = "typing",
    params(("tenant_id" = Uuid, Path), ("id" = Uuid, Path)),
    responses((status = 204), (status = 404)),
)]
async fn typing(
    Extension(messaging): Extension<Messaging>,
    Extension(token): Extension<AccessToken>,
    Input(key): Input<Key>,
) -> Result<StatusCode, MessagingError> {
    let speaker = messaging.speaker(key.tenant_id, key.id, token.sub).await?;
    messaging.typing(key.tenant_id, key.id, speaker.id).await?;
    Ok(StatusCode::NO_CONTENT)
}
