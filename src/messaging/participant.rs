//! Participant: a member or a client taking part in one thread.

use axum::Extension;
use axum::extract::State;
use serde::{Deserialize, Serialize};
use sqlx::PgExecutor;
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use uuid::Uuid;
use validator::Validate;

use super::error::MessagingError;
use super::hub::{Messaging, Paged};
use super::thread;
use crate::Entity;
use crate::auth::AccessToken;
use crate::auth::rest::{AppContext, Input, Page, fetch_page};

/// Who takes part: a tenant member (a provider, staff, an AI agent) or a client.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Party {
    Member(Uuid),
    Client(Uuid),
}

impl Party {
    fn id(self) -> Uuid {
        match self {
            Self::Member(id) | Self::Client(id) => id,
        }
    }

    fn columns(self) -> (Option<Uuid>, Option<Uuid>) {
        match self {
            Self::Member(id) => (Some(id), None),
            Self::Client(id) => (None, Some(id)),
        }
    }
}

/// Exactly one of `member_id` and `client_id` is set.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, sqlx::FromRow, ToSchema)]
pub struct ParticipantData {
    pub tenant_id: Uuid,
    pub thread_id: Uuid,
    pub member_id: Option<Uuid>,
    pub client_id: Option<Uuid>,
    pub last_read_message_id: Option<Uuid>,
}

pub type Participant = Entity<Uuid, ParticipantData>;

impl Messaging {
    /// `party` as a participant of the thread; the one it already is when it has joined.
    pub async fn join(&self, tenant_id: Uuid, thread_id: Uuid, party: Party) -> Result<Participant, MessagingError> {
        match insert(&self.pool, tenant_id, thread_id, party).await? {
            Some(joined) => Ok(joined),
            None => existing(&self.pool, tenant_id, thread_id, party.id()).await?.ok_or(MessagingError::NotFound),
        }
    }

    /// The participant of `subject` (a token's `sub`); a tenant member who is not one yet joins.
    pub async fn speaker(&self, tenant_id: Uuid, thread_id: Uuid, subject: Uuid) -> Result<Participant, MessagingError> {
        match existing(&self.pool, tenant_id, thread_id, subject).await? {
            Some(speaker) => Ok(speaker),
            None => self.join(tenant_id, thread_id, Party::Member(subject)).await,
        }
    }

    /// `subject` as a member when it is one of the tenant's, else as a client.
    pub async fn party(&self, tenant_id: Uuid, subject: Uuid) -> Result<Party, MessagingError> {
        let member = sqlx::query_scalar::<_, bool>("SELECT EXISTS (SELECT 1 FROM tenant_members WHERE tenant_id = $1 AND id = $2)")
            .bind(tenant_id)
            .bind(subject)
            .fetch_one(&self.pool)
            .await?;
        Ok(member.then_some(Party::Member(subject)).unwrap_or(Party::Client(subject)))
    }
}

/// Nothing when the thread is missing, the member is not the tenant's, the client is missing, or `party` is already in.
pub(super) async fn insert<'e>(
    db: impl PgExecutor<'e>,
    tenant_id: Uuid,
    thread_id: Uuid,
    party: Party,
) -> Result<Option<Participant>, sqlx::Error> {
    let (member_id, client_id) = party.columns();
    sqlx::query_as(
        "INSERT INTO participants (tenant_id, id, thread_id, member_id, client_id)
         SELECT $1, $2, t.id, $4, $5 FROM threads t
         WHERE t.tenant_id = $1 AND t.id = $3 AND t.deleted_at IS NULL
           AND ($4::uuid IS NULL OR EXISTS (SELECT 1 FROM tenant_members m WHERE m.tenant_id = $1 AND m.id = $4))
           AND ($5::uuid IS NULL OR EXISTS (SELECT 1 FROM clients c WHERE c.tenant_id = $1 AND c.id = $5 AND c.deleted_at IS NULL))
         ON CONFLICT DO NOTHING RETURNING *",
    )
    .bind(tenant_id)
    .bind(Uuid::now_v7())
    .bind(thread_id)
    .bind(member_id)
    .bind(client_id)
    .fetch_optional(db)
    .await
}

async fn existing<'e>(db: impl PgExecutor<'e>, tenant_id: Uuid, thread_id: Uuid, subject: Uuid) -> Result<Option<Participant>, sqlx::Error> {
    sqlx::query_as(
        "SELECT * FROM participants WHERE tenant_id = $1 AND thread_id = $2 AND (member_id = $3 OR client_id = $3) AND deleted_at IS NULL",
    )
    .bind(tenant_id)
    .bind(thread_id)
    .bind(subject)
    .fetch_optional(db)
    .await
}

pub(super) async fn require<'e>(db: impl PgExecutor<'e>, tenant_id: Uuid, thread_id: Uuid, participant_id: Uuid) -> Result<(), MessagingError> {
    sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM participants WHERE tenant_id = $1 AND thread_id = $2 AND id = $3 AND deleted_at IS NULL)",
    )
    .bind(tenant_id)
    .bind(thread_id)
    .bind(participant_id)
    .fetch_one(db)
    .await?
    .then_some(())
    .ok_or(MessagingError::NotFound)
}

pub fn routes() -> OpenApiRouter<AppContext> {
    OpenApiRouter::new().routes(routes!(list, join))
}

/// Who joins: `{"member_id"}` or `{"client_id"}`; `{}` is the caller.
#[derive(Debug, Deserialize, Validate, ToSchema)]
struct Joining {
    tenant_id: Uuid,
    id: Uuid,
    member_id: Option<Uuid>,
    client_id: Option<Uuid>,
}

#[utoipa::path(
    get,
    path = "/tenants/{tenant_id}/threads/{id}/participants",
    tag = "participants",
    params(("tenant_id" = Uuid, Path), ("id" = Uuid, Path), ("page_size" = Option<u16>, Query), ("page_token" = Option<String>, Query)),
    responses((status = 200, body = inline(Page<Entity<Uuid, ParticipantData>>)), (status = 404)),
)]
async fn list(State(ctx): State<AppContext>, Input(paged): Input<Paged>) -> Result<Page<Participant>, MessagingError> {
    thread::require(&ctx.pool, paged.tenant_id, paged.id).await?;
    Ok(fetch_page(&ctx.pool, in_thread(&paged), &paged.page).await?)
}

fn in_thread(paged: &Paged) -> sqlx::QueryBuilder<'static, sqlx::Postgres> {
    let mut query = sqlx::QueryBuilder::new("SELECT * FROM participants WHERE deleted_at IS NULL AND tenant_id = ");
    query.push_bind(paged.tenant_id).push(" AND thread_id = ").push_bind(paged.id);
    query
}

#[utoipa::path(
    post,
    path = "/tenants/{tenant_id}/threads/{id}/participants",
    tag = "participants",
    params(("tenant_id" = Uuid, Path), ("id" = Uuid, Path)),
    request_body = Joining,
    responses((status = 200, body = inline(Entity<Uuid, ParticipantData>)), (status = 400), (status = 404)),
)]
async fn join(
    Extension(messaging): Extension<Messaging>,
    Extension(token): Extension<AccessToken>,
    Input(joining): Input<Joining>,
) -> Result<Participant, MessagingError> {
    let (tenant_id, thread_id) = (joining.tenant_id, joining.id);
    match (joining.member_id, joining.client_id) {
        (Some(member), None) => messaging.join(tenant_id, thread_id, Party::Member(member)).await,
        (None, Some(client)) => messaging.join(tenant_id, thread_id, Party::Client(client)).await,
        (None, None) => messaging.speaker(tenant_id, thread_id, token.sub).await,
        (Some(_), Some(_)) => Err(MessagingError::BadRequest("give a member_id or a client_id, not both".into())),
    }
}
