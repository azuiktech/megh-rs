//! Thread: the container of a conversation between its participants.

use axum::Extension;
use axum::extract::State;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::{PgConnection, PgExecutor};
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use uuid::Uuid;
use validator::Validate;

use super::error::MessagingError;
use super::messaging::{Messaging, threads_topic};
use super::participant::{self, Participant, Party};
use crate::auth::AccessToken;
use crate::auth::crud;
use crate::auth::rest::{ApiResult, AppContext, Input, Key, ListRequest, Page};
use crate::booking::value::Name;
use crate::event::Event;
use crate::{Entity, Table};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, sqlx::Type, ToSchema)]
#[sqlx(type_name = "text", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum ThreadStatus {
    #[default]
    Open,
    Closed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Validate, sqlx::FromRow, Table, ToSchema)]
#[table(name = "threads", keys = ["tenant_id", "id"])]
pub struct ThreadData {
    pub tenant_id: Uuid,
    #[serde(default)]
    #[schema(value_type = Option<String>, example = "Haircut question")]
    pub subject: Option<Name>,
    #[serde(default)]
    pub status: ThreadStatus,
    #[serde(default)]
    pub appointment_id: Option<Uuid>,
    #[serde(default)]
    pub invoice_id: Option<Uuid>,
    #[serde(default)]
    pub last_message_at: Option<DateTime<Utc>>,
}

pub type Thread = Entity<Uuid, ThreadData>;

impl Messaging {
    /// A new thread in which `opener` is the first participant; it is published on the tenant's thread list.
    pub async fn open(&self, tenant_id: Uuid, opener: Party, subject: Option<Name>) -> Result<Thread, MessagingError> {
        let mut transaction = self.pool.begin().await?;
        let (thread, _) = start(&mut transaction, tenant_id, opener, subject).await?;
        transaction.commit().await?;
        self.announce_thread(&thread).await;
        Ok(thread)
    }

    pub(super) async fn announce_thread(&self, thread: &Thread) {
        self.announce(Event::new(threads_topic(thread.tenant_id), json!({"kind": "thread", "thread": thread}))).await;
    }
}

/// A thread and its first participant, `opener`.
pub(super) async fn start(conn: &mut PgConnection, tenant_id: Uuid, opener: Party, subject: Option<Name>) -> Result<(Thread, Participant), MessagingError> {
    let thread: Thread = sqlx::query_as("INSERT INTO threads (tenant_id, id, subject) VALUES ($1, $2, $3) RETURNING *")
        .bind(tenant_id)
        .bind(Uuid::now_v7())
        .bind(subject)
        .fetch_one(&mut *conn)
        .await?;
    let first = participant::insert(conn, tenant_id, thread.id, opener).await?.ok_or(MessagingError::NotFound)?;
    Ok((thread, first))
}

pub(super) async fn require<'e>(db: impl PgExecutor<'e>, tenant_id: Uuid, thread_id: Uuid) -> Result<(), MessagingError> {
    sqlx::query_scalar::<_, bool>("SELECT EXISTS (SELECT 1 FROM threads WHERE tenant_id = $1 AND id = $2 AND deleted_at IS NULL)")
        .bind(tenant_id)
        .bind(thread_id)
        .fetch_one(db)
        .await?
        .then_some(())
        .ok_or(MessagingError::NotFound)
}

pub fn routes() -> OpenApiRouter<AppContext> {
    OpenApiRouter::new().routes(routes!(list, create)).routes(routes!(get))
}

#[derive(Debug, Deserialize, Validate, ToSchema)]
struct NewThread {
    tenant_id: Uuid,
    #[schema(value_type = Option<String>, example = "Haircut question")]
    subject: Option<Name>,
}

#[utoipa::path(
    get,
    path = "/tenants/{tenant_id}/threads",
    tag = "threads",
    params(
        ("tenant_id" = Uuid, Path),
        ("filter" = Option<String>, Query, description = "AIP-160 filter (https://google.aip.dev/160) on any field of the entity."),
        ("page_size" = Option<u16>, Query),
        ("page_token" = Option<String>, Query),
    ),
    responses((status = 200, body = inline(Page<Entity<Uuid, ThreadData>>))),
)]
async fn list(State(ctx): State<AppContext>, Input(request): Input<ListRequest>) -> ApiResult<Page<Thread>> {
    crud::list(&ctx.pool, &request).await
}

#[utoipa::path(
    get,
    path = "/tenants/{tenant_id}/threads/{id}",
    tag = "threads",
    params(("tenant_id" = Uuid, Path), ("id" = Uuid, Path)),
    responses((status = 200, body = inline(Entity<Uuid, ThreadData>)), (status = 404)),
)]
async fn get(State(ctx): State<AppContext>, Input(key): Input<Key>) -> ApiResult<Thread> {
    crud::get(&ctx.pool, &key).await
}

/// Opens a thread with the caller as its first participant.
#[utoipa::path(
    post,
    path = "/tenants/{tenant_id}/threads",
    tag = "threads",
    params(("tenant_id" = Uuid, Path)),
    request_body = NewThread,
    responses((status = 200, body = inline(Entity<Uuid, ThreadData>))),
)]
async fn create(
    Extension(messaging): Extension<Messaging>,
    Extension(token): Extension<AccessToken>,
    Input(new): Input<NewThread>,
) -> Result<Thread, MessagingError> {
    let opener = messaging.party(new.tenant_id, token.sub).await?;
    messaging.open(new.tenant_id, opener, new.subject).await
}
