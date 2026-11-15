//! Message: what a participant says in a thread: text, a2ui surface, or the answer to a surface.

use axum::Extension;
use axum::routing::post;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::types::Json;
use sqlx::{PgConnection, PgExecutor, Postgres, QueryBuilder};
use utoipa::{OpenApi, ToSchema};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use uuid::Uuid;
use validator::Validate;

use super::attachment::AttachmentSummary;
use super::error::MessagingError;
use super::hub::{Messaging, Paged, thread_topic};
use super::surface::{Action, Surface};
use super::thread;
use crate::Entity;
use crate::auth::AccessToken;
use crate::auth::rest::{AppContext, Input, Key, Page, PageRequest, fetch_page};
use crate::event::Event;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type, ToSchema)]
#[sqlx(type_name = "text", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum MessageKind {
    Text,
    Surface,
    Answer,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, sqlx::FromRow, ToSchema)]
pub struct MessageData {
    pub tenant_id: Uuid,
    pub thread_id: Uuid,
    /// The participant who sent it.
    pub sender_id: Uuid,
    pub kind: MessageKind,
    pub text: Option<String>,
    /// The a2ui 1.0 messages of a surface.
    #[sqlx(json(nullable))]
    #[schema(value_type = Option<Vec<Object>>)]
    pub surface: Option<Vec<Value>>,
    pub surface_id: Option<String>,
    pub in_reply_to: Option<Uuid>,
    /// The a2ui action of an answer.
    #[sqlx(json(nullable))]
    #[schema(value_type = Option<Object>)]
    pub answer: Option<Action>,
    #[sqlx(json)]
    pub attachments: Vec<AttachmentSummary>,
}

pub type Message = Entity<Uuid, MessageData>;

/// What to say.
#[derive(Debug, Clone, PartialEq)]
pub enum Content {
    Text { text: String, attachments: Vec<Uuid> },
    Surface(Surface),
    Answer { surface_id: String, action: Action },
}

impl Content {
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text { text: text.into(), attachments: Vec::new() }
    }

    pub fn surface(surface: Surface) -> Self {
        Self::Surface(surface)
    }
}

/// A message about to be stored.
struct Draft {
    tenant_id: Uuid,
    thread_id: Uuid,
    sender: Uuid,
    in_reply_to: Option<Uuid>,
    kind: MessageKind,
    text: Option<String>,
    surface: Option<Json<Vec<Value>>>,
    surface_id: Option<String>,
    answer: Option<Json<Action>>,
    attachments: Vec<Uuid>,
}

impl Draft {
    fn new(tenant_id: Uuid, thread_id: Uuid, sender: Uuid, content: Content) -> Self {
        let empty = Self {
            tenant_id,
            thread_id,
            sender,
            in_reply_to: None,
            kind: MessageKind::Text,
            text: None,
            surface: None,
            surface_id: None,
            answer: None,
            attachments: Vec::new(),
        };
        match content {
            Content::Text { text, attachments } => Self { text: Some(text), attachments, ..empty },
            Content::Surface(surface) => Self { kind: MessageKind::Surface, surface: Some(Json(surface.messages)), surface_id: Some(surface.id), ..empty },
            Content::Answer { surface_id, action } => {
                Self { kind: MessageKind::Answer, text: action.user_message.clone(), surface_id: Some(surface_id), answer: Some(Json(action)), ..empty }
            }
        }
    }
}

impl Messaging {
    pub async fn send(&self, tenant_id: Uuid, thread_id: Uuid, sender: Uuid, content: Content) -> Result<Message, MessagingError> {
        self.post(Draft::new(tenant_id, thread_id, sender, content)).await
    }

    /// `send`, answering the message `to`.
    pub async fn reply(&self, tenant_id: Uuid, thread_id: Uuid, sender: Uuid, to: Uuid, content: Content) -> Result<Message, MessagingError> {
        self.post(Draft { in_reply_to: Some(to), ..Draft::new(tenant_id, thread_id, sender, content) }).await
    }

    /// Stores the user's action on the surface message `surface_message` as an answer to it.
    pub async fn respond(&self, tenant_id: Uuid, thread_id: Uuid, sender: Uuid, surface_message: Uuid, action: Action) -> Result<Message, MessagingError> {
        let surface_id: String = sqlx::query_scalar(
            "SELECT surface_id FROM messages WHERE tenant_id = $1 AND thread_id = $2 AND id = $3 AND kind = 'surface' AND surface_id IS NOT NULL AND deleted_at IS NULL",
        )
        .bind(tenant_id)
        .bind(thread_id)
        .bind(surface_message)
        .fetch_optional(&self.pool)
        .await?
        .ok_or(MessagingError::NotFound)?;
        self.reply(tenant_id, thread_id, sender, surface_message, Content::Answer { surface_id, action }).await
    }

    pub async fn messages(&self, tenant_id: Uuid, thread_id: Uuid, page: PageRequest) -> Result<Page<Message>, MessagingError> {
        thread::require(&self.pool, tenant_id, thread_id).await?;
        let mut query = select(tenant_id);
        query.push(" AND m.thread_id = ").push_bind(thread_id);
        Ok(fetch_page(&self.pool, query, &page).await?)
    }

    /// Stored first, then published on the thread's topic.
    async fn post(&self, draft: Draft) -> Result<Message, MessagingError> {
        let mut transaction = self.pool.begin().await?;
        let id = insert(&mut transaction, &draft).await?;
        claim(&mut transaction, id, &draft).await?;
        sqlx::query("UPDATE threads SET last_message_at = now(), updated_at = now() WHERE tenant_id = $1 AND id = $2")
            .bind(draft.tenant_id)
            .bind(draft.thread_id)
            .execute(&mut *transaction)
            .await?;
        let message = fetch(&mut *transaction, draft.tenant_id, id).await?;
        transaction.commit().await?;
        self.announce(message_event(&message)).await;
        Ok(message)
    }
}

/// A message as an event: its id is the event id, so a client that reconnects asks for what came after it.
pub(super) fn message_event(message: &Message) -> Event {
    Event::new(thread_topic(message.tenant_id, message.thread_id), json!({"kind": "message", "message": message})).with_id(message.id.to_string())
}

/// Messages of a tenant with the attachments each carries; the caller adds conditions with `AND`.
pub(super) fn select(tenant_id: Uuid) -> QueryBuilder<'static, Postgres> {
    let mut query = QueryBuilder::new(
        "SELECT m.*, COALESCE((SELECT json_agg(json_build_object('id', a.id, 'filename', a.filename, 'content_type', a.content_type, 'size', a.size) ORDER BY a.id)
                               FROM attachments a WHERE a.tenant_id = m.tenant_id AND a.message_id = m.id AND a.deleted_at IS NULL), '[]'::json) AS attachments
         FROM messages m WHERE m.deleted_at IS NULL AND m.tenant_id = ",
    );
    query.push_bind(tenant_id);
    query
}

async fn insert(conn: &mut PgConnection, draft: &Draft) -> Result<Uuid, MessagingError> {
    sqlx::query_scalar(
        "INSERT INTO messages (tenant_id, id, thread_id, sender_id, kind, text, surface, surface_id, in_reply_to, answer)
         SELECT $1, $2, p.thread_id, p.id, $4, $5, $6, $7,
                COALESCE($8, (SELECT s.id FROM messages s WHERE s.tenant_id = $1 AND s.thread_id = $3 AND s.kind = 'surface' AND s.surface_id = $7 AND s.deleted_at IS NULL LIMIT 1)),
                $9
         FROM participants p WHERE p.tenant_id = $1 AND p.thread_id = $3 AND p.id = $10 AND p.deleted_at IS NULL
         RETURNING id",
    )
    .bind(draft.tenant_id)
    .bind(Uuid::now_v7())
    .bind(draft.thread_id)
    .bind(draft.kind)
    .bind(&draft.text)
    .bind(&draft.surface)
    .bind(&draft.surface_id)
    .bind(draft.in_reply_to)
    .bind(&draft.answer)
    .bind(draft.sender)
    .fetch_optional(conn)
    .await?
    .ok_or(MessagingError::NotFound)
}

/// Attaches the files to the message; each must be of the same thread and not yet attached.
async fn claim(conn: &mut PgConnection, id: Uuid, draft: &Draft) -> Result<(), MessagingError> {
    let claimed = sqlx::query(
        "UPDATE attachments SET message_id = $3, updated_at = now()
         WHERE tenant_id = $1 AND thread_id = $2 AND id = ANY($4) AND message_id IS NULL AND deleted_at IS NULL",
    )
    .bind(draft.tenant_id)
    .bind(draft.thread_id)
    .bind(id)
    .bind(&draft.attachments)
    .execute(conn)
    .await?
    .rows_affected();
    (claimed == draft.attachments.len() as u64).then_some(()).ok_or(MessagingError::NotFound)
}

async fn fetch<'e>(db: impl PgExecutor<'e>, tenant_id: Uuid, id: Uuid) -> Result<Message, MessagingError> {
    let mut query = select(tenant_id);
    query.push(" AND m.id = ").push_bind(id);
    Ok(query.build_query_as().fetch_one(db).await?)
}

#[derive(OpenApi)]
#[openapi(paths(respond))]
struct Actions;

pub fn routes() -> OpenApiRouter<AppContext> {
    OpenApiRouter::with_openapi(Actions::openapi())
        .routes(routes!(list, create))
        .route("/tenants/{tenant_id}/threads/{id}/messages/{mid}/{verb}", post(respond))
}

/// The body of a message: `surface` (a2ui 1.0 messages) makes it a surface, else it is text.
#[derive(Debug, Deserialize, ToSchema)]
struct NewMessage {
    text: Option<String>,
    #[serde(default)]
    attachments: Vec<Uuid>,
    #[schema(value_type = Option<Vec<Object>>)]
    surface: Option<Vec<Value>>,
    in_reply_to: Option<Uuid>,
}

impl NewMessage {
    fn content(self) -> Result<Content, MessagingError> {
        match self.surface {
            Some(messages) => Surface::generic(messages).map(Content::Surface),
            None => Ok(Content::Text { text: self.text.unwrap_or_default(), attachments: self.attachments }),
        }
    }
}

#[derive(Debug, Deserialize, Validate)]
struct Posting {
    #[serde(flatten)]
    key: Key,
    #[serde(flatten)]
    message: NewMessage,
}

/// The path segment of the custom method.
#[derive(Debug, Deserialize)]
enum Verb {
    #[serde(rename = ":respond")]
    Respond,
}

#[derive(Debug, Deserialize, Validate)]
struct Responding {
    tenant_id: Uuid,
    id: Uuid,
    mid: Uuid,
    #[serde(rename = "verb")]
    _verb: Verb,
    #[serde(flatten)]
    action: Action,
}

#[utoipa::path(
    get,
    path = "/tenants/{tenant_id}/threads/{id}/messages",
    tag = "messages",
    params(("tenant_id" = Uuid, Path), ("id" = Uuid, Path), ("page_size" = Option<u16>, Query), ("page_token" = Option<String>, Query)),
    responses((status = 200, body = inline(Page<Entity<Uuid, MessageData>>)), (status = 404)),
)]
async fn list(Extension(messaging): Extension<Messaging>, Input(paged): Input<Paged>) -> Result<Page<Message>, MessagingError> {
    messaging.messages(paged.tenant_id, paged.id, paged.page).await
}

/// Sends the message as the caller, who joins the thread when a tenant member who has not yet.
#[utoipa::path(
    post,
    path = "/tenants/{tenant_id}/threads/{id}/messages",
    tag = "messages",
    params(("tenant_id" = Uuid, Path), ("id" = Uuid, Path)),
    request_body = NewMessage,
    responses((status = 200, body = inline(Entity<Uuid, MessageData>)), (status = 400), (status = 404)),
)]
async fn create(
    Extension(messaging): Extension<Messaging>,
    Extension(token): Extension<AccessToken>,
    Input(posting): Input<Posting>,
) -> Result<Message, MessagingError> {
    let Key { tenant_id, id } = posting.key;
    let sender = messaging.speaker(tenant_id, id, token.sub).await?;
    let in_reply_to = posting.message.in_reply_to;
    let content = posting.message.content()?;
    match in_reply_to {
        Some(to) => messaging.reply(tenant_id, id, sender.id, to, content).await,
        None => messaging.send(tenant_id, id, sender.id, content).await,
    }
}

/// The user's a2ui action on a surface message, stored as an answer to it.
#[utoipa::path(
    post,
    path = "/tenants/{tenant_id}/threads/{id}/messages/{mid}:respond",
    tag = "messages",
    params(("tenant_id" = Uuid, Path), ("id" = Uuid, Path), ("mid" = Uuid, Path)),
    request_body(content = Object, description = "an a2ui action: `{\"name\", \"context\", \"userMessage\"?}`"),
    responses((status = 200, body = inline(Entity<Uuid, MessageData>)), (status = 404)),
)]
async fn respond(
    Extension(messaging): Extension<Messaging>,
    Extension(token): Extension<AccessToken>,
    Input(responding): Input<Responding>,
) -> Result<Message, MessagingError> {
    let sender = messaging.speaker(responding.tenant_id, responding.id, token.sub).await?;
    messaging.respond(responding.tenant_id, responding.id, sender.id, responding.mid, responding.action).await
}
