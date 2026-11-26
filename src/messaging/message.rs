//! Messaging between a client and a member of one tenant: a message is stored, then published on the pair's topic;
//! `on` and `on_event` listen on the broker; `History` lets a reconnecting browser catch up from the database.

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use chrono::{DateTime, Utc};
use futures_util::StreamExt;
use futures_util::future::ready;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use crate::Db;
use sqlx::types::Json as Column;
use tokio_stream::wrappers::ReceiverStream;
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use uuid::Uuid;
use validator::Validate;

use super::surface::{Frame, Surface};
use crate::auth::AccessToken;
use crate::auth::rest::{ApiError, AppContext, Input};
use crate::event::{Broker, Event, HistoryError, HistoryFuture, HistoryOptions, HistoryProvider, Topic};

/// Who talks: a member of the tenant (staff, a provider, an agent) or a client (a guest, a prospect, a client).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
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
}

/// What is said without a message: typing, that messages up to one were read, and that a guest is now this client.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Signal {
    TypingOn,
    TypingOff,
    Read(Uuid),
    Moved(Uuid),
}

/// What stands for a sender beside their messages.
pub enum Avatar {
    Image(String),
    Vector(String),
    Text(String),
}

/// An icon the client draws: a name it maps to its own icon set, or an SVG path.
#[derive(Debug, Clone)]
pub enum Icon {
    Name(String),
    Svg(String),
}

impl Icon {
    pub fn name(name: impl Into<String>) -> Self {
        Self::Name(name.into())
    }

    fn value(&self) -> Value {
        match self {
            Self::Name(name) => json!(name),
            Self::Svg(path) => json!({"svgPath": path}),
        }
    }
}

/// A `chrono` format for the time shown under a message.
#[derive(Debug, Clone, Copy)]
pub struct TimeFormat(pub &'static str);

/// The icons of a message that is sent and of one that is read.
#[derive(Debug, Clone)]
pub struct ReadMark {
    pub sent: Icon,
    pub read: Icon,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, ToSchema)]
pub struct Message {
    pub id: Uuid,
    pub from: Party,
    pub to: Party,
    /// a2ui 1.0 messages, or the action a renderer sent back.
    #[schema(value_type = Object)]
    pub body: Value,
    pub created_at: DateTime<Utc>,
}

/// An event between the two sides; a read carries the a2ui updates that turn the read mark.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Notice {
    pub from: Party,
    pub to: Party,
    pub signal: Signal,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub updates: Vec<Value>,
}

/// What travels on a pair's topic.
#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Delivery {
    Message(Message),
    Event(Notice),
}

impl Delivery {
    fn from(&self) -> Party {
        match self {
            Self::Message(message) => message.from,
            Self::Event(notice) => notice.from,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum MessagingError {
    #[error("messages go between a client and a member")]
    NotAPair,
    #[error("no such file")]
    NotFound,
    #[error(transparent)]
    File(#[from] std::io::Error),
    #[error(transparent)]
    Session(#[from] tower_sessions::session::Error),
    #[error(transparent)]
    Token(#[from] jsonwebtoken::errors::Error),
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}

impl IntoResponse for MessagingError {
    fn into_response(self) -> Response {
        match self {
            Self::NotAPair => (StatusCode::BAD_REQUEST, self.to_string()).into_response(),
            Self::NotFound => (StatusCode::NOT_FOUND, self.to_string()).into_response(),
            Self::File(_) | Self::Session(_) | Self::Token(_) => (StatusCode::INTERNAL_SERVER_ERROR, self.to_string()).into_response(),
            Self::Database(error) => ApiError::Database(error).into_response(),
        }
    }
}

/// The topic of a client and a member of a tenant; a missing side is any.
fn topic(tenant_id: Uuid, client: Option<Uuid>, member: Option<Uuid>) -> Topic {
    let any = |id: Option<Uuid>| id.map_or("*".to_string(), |id| id.to_string());
    Topic::new(format!("tenants/{tenant_id}/clients/{}/members/{}", any(client), any(member)))
}

/// The client, the member, and which of them sends.
pub(super) fn pair(from: Party, to: Party) -> Result<(Uuid, Uuid, &'static str), MessagingError> {
    match (from, to) {
        (Party::Client(client), Party::Member(member)) => Ok((client, member, "client")),
        (Party::Member(member), Party::Client(client)) => Ok((client, member, "member")),
        _ => Err(MessagingError::NotAPair),
    }
}

#[derive(sqlx::FromRow)]
struct Row {
    tenant_id: Uuid,
    id: Uuid,
    client_id: Uuid,
    member_id: Uuid,
    sender: String,
    body: Column<Value>,
    created_at: DateTime<Utc>,
}

impl Row {
    fn delivered(self) -> (Message, Event) {
        let (client, member) = (Party::Client(self.client_id), Party::Member(self.member_id));
        let (from, to) = match self.sender.as_str() {
            "member" => (member, client),
            _ => (client, member),
        };
        let message = Message { id: self.id, from, to, body: self.body.0, created_at: self.created_at };
        let data = serde_json::to_value(Delivery::Message(message.clone())).unwrap_or_default();
        let event = Event::new(topic(self.tenant_id, Some(self.client_id), Some(self.member_id)), data).with_id(self.id.to_string());
        (message, Event { timestamp: self.created_at, ..event })
    }
}

#[derive(Clone)]
pub struct Messaging {
    pub(super) pool: Db,
    broker: Broker,
    pub(super) tenant_id: Uuid,
    pub(super) files_at: String,
    avatar: Option<fn(Uuid) -> Avatar>,
    timestamp: Option<TimeFormat>,
    read_mark: Option<ReadMark>,
}

impl Messaging {
    /// Messaging within one tenant: nothing it sends or hears belongs to another.
    pub fn new(pool: Db, broker: Broker, tenant_id: Uuid) -> Self {
        Self { pool, broker, tenant_id, files_at: String::new(), avatar: None, timestamp: None, read_mark: None }
    }

    /// The avatar shown beside each message, from the id of its sender.
    pub fn with_avatar(self, avatar: fn(Uuid) -> Avatar) -> Self {
        Self { avatar: Some(avatar), ..self }
    }

    pub fn with_timestamp(self, format: TimeFormat) -> Self {
        Self { timestamp: Some(format), ..self }
    }

    pub fn with_read_mark(self, mark: ReadMark) -> Self {
        Self { read_mark: Some(mark), ..self }
    }

    /// Where the routes are mounted (`https://app.example.com/api/v1`), so a file's `fileUri` loads from anywhere.
    pub fn with_files_at(self, base: &str) -> Self {
        Self { files_at: base.trim_end_matches('/').to_string(), ..self }
    }

    /// Sends the surface, framed as configured, from one side of a client and a member to the other.
    pub async fn send(&self, surface: Surface, from: Party, to: Party) -> Result<Message, MessagingError> {
        let body = surface.framed(&Uuid::now_v7().to_string(), &self.frame(from.id()));
        self.store(json!(body), from, to).await
    }

    /// Tells the other side something that is not a message; a read turns the read mark of what was read.
    pub async fn send_event(&self, signal: Signal, from: Party, to: Party) -> Result<(), MessagingError> {
        let (client, member, _) = pair(from, to)?;
        let updates = match signal {
            Signal::Read(up_to) => self.read(client, member, from, up_to).await?,
            Signal::TypingOn | Signal::TypingOff | Signal::Moved(_) => Vec::new(),
        };
        let data = serde_json::to_value(Delivery::Event(Notice { from, to, signal, updates })).unwrap_or_default();
        self.broker.publish(Event::new(topic(self.tenant_id, Some(client), Some(member)), data)).await;
        Ok(())
    }

    /// Calls `listener` with every message sent to `me`.
    pub fn on(&self, me: Party, listener: impl Fn(Message) + Send + 'static) {
        self.listen(me, move |delivery| match delivery {
            Delivery::Message(message) => listener(message),
            Delivery::Event(_) => (),
        })
    }

    /// Calls `listener` with every event sent to `me`.
    pub fn on_event(&self, me: Party, listener: impl Fn(Notice) + Send + 'static) {
        self.listen(me, move |delivery| match delivery {
            Delivery::Event(notice) => listener(notice),
            Delivery::Message(_) => (),
        })
    }

    fn frame(&self, sender: Uuid) -> Frame {
        Frame {
            avatar: self.avatar.map(|avatar| avatar_component(avatar(sender))),
            time: self.timestamp.map(|format| Utc::now().format(format.0).to_string()),
            mark: self.read_mark.as_ref().map(|mark| (mark.sent.value(), mark.read.value())),
        }
    }

    async fn store(&self, body: Value, from: Party, to: Party) -> Result<Message, MessagingError> {
        let (client, member, sender) = pair(from, to)?;
        let row: Row = Db::query_as(
            "INSERT INTO messages (tenant_id, id, client_id, member_id, sender, body) VALUES ($1, $2, $3, $4, $5, $6) RETURNING *",
        )
        .bind(self.tenant_id)
        .bind(Uuid::now_v7())
        .bind(client)
        .bind(member)
        .bind(sender)
        .bind(Column(body))
        .fetch_one(&self.pool)
        .await?;
        let (message, event) = row.delivered();
        self.broker.publish(event).await;
        Ok(message)
    }

    /// Marks read what the other side sent `reader` up to `up_to`; answers the a2ui updates of their read marks.
    async fn read(&self, client: Uuid, member: Uuid, reader: Party, up_to: Uuid) -> Result<Vec<Value>, sqlx::Error> {
        let sender = match reader {
            Party::Client(_) => "member",
            Party::Member(_) => "client",
        };
        let marked: Vec<(Option<String>, Option<Column<Value>>)> = Db::query_as(
            "UPDATE messages SET read_at = now(),
                    body = COALESCE(jsonb_set(body, '{0,createSurface,dataModel,meta,mark}', body #> '{0,createSurface,dataModel,meta,read}'), body)
             WHERE tenant_id = $1 AND client_id = $2 AND member_id = $3 AND sender = $4 AND id <= $5 AND read_at IS NULL
             RETURNING body #>> '{0,createSurface,surfaceId}', body #> '{0,createSurface,dataModel,meta,read}'",
        )
        .sqlite(concat!(
            "UPDATE messages SET read_at = ", crate::sqlite_now!(), ",
                    body = CASE WHEN body -> '$[0].createSurface.dataModel.meta.read' IS NULL THEN body
                                ELSE json_set(body, '$[0].createSurface.dataModel.meta.mark', json(body -> '$[0].createSurface.dataModel.meta.read')) END
             WHERE tenant_id = $1 AND client_id = $2 AND member_id = $3 AND sender = $4 AND id <= $5 AND read_at IS NULL
             RETURNING body ->> '$[0].createSurface.surfaceId', body -> '$[0].createSurface.dataModel.meta.read'",
        ))
        .bind(self.tenant_id)
        .bind(client)
        .bind(member)
        .bind(sender)
        .bind(up_to)
        .fetch_all(&self.pool)
        .await?;
        Ok(marked.into_iter().filter_map(|(id, read)| Some(read_update(id?, read?.0))).collect())
    }

    /// Who `me` is to the broker's authorizer: `me` in this tenant, never sent anywhere, so it is not signed.
    fn listener(&self, me: Party) -> AccessToken {
        let tenant = self.tenant_id.to_string();
        AccessToken { sub: me.id(), iss: "megh".into(), aud: vec![tenant], exp: u64::MAX, iat: 0, client_id: "megh".into(), scope: String::new() }
    }

    /// Follows `me`'s topics on the broker and hands each delivery from the other side to `deliver`.
    fn listen(&self, me: Party, deliver: impl Fn(Delivery) + Send + 'static) {
        let pattern = match me {
            Party::Client(client) => topic(self.tenant_id, Some(client), None),
            Party::Member(member) => topic(self.tenant_id, None, Some(member)),
        };
        let (broker, owner) = (self.broker.clone(), self.listener(me));
        tokio::spawn(async move {
            let name = Uuid::now_v7().to_string();
            let received = broker.register(&name, Some(owner)).await;
            let _ = broker.subscribe(&name, &[pattern]).await;
            ReceiverStream::new(received)
                .filter_map(|event| ready(serde_json::from_value::<Delivery>(event.data).ok()))
                .filter(move |delivery| ready(delivery.from() != me))
                .for_each(move |delivery| {
                    deliver(delivery);
                    ready(())
                })
                .await;
        });
    }
}

fn avatar_component(avatar: Avatar) -> Value {
    match avatar {
        Avatar::Image(url) => json!({"id": "avatar", "component": "Image", "url": url, "variant": "avatar"}),
        Avatar::Vector(path) => json!({"id": "avatar", "component": "Icon", "name": {"svgPath": path}}),
        Avatar::Text(text) => json!({"id": "avatar", "component": "Text", "text": text}),
    }
}

fn read_update(surface_id: String, icon: Value) -> Value {
    json!({"version": "1.0", "updateDataModel": {"surfaceId": surface_id, "path": "/meta/mark", "value": icon}})
}

/// Which end of the messages `limit` keeps: the oldest (catching up after an id) or the latest (a first look).
enum End {
    Oldest,
    Latest,
}

/// Which messages: those after an id or a time, at most `limit` (none is every one) from one end.
struct Window {
    after: Option<Uuid>,
    since: Option<DateTime<Utc>>,
    limit: Option<usize>,
    end: End,
}

/// The broker's `HistoryProvider` for messaging topics, read from the stored messages; other topics have none.
#[derive(Clone)]
pub struct History {
    pool: Db,
}

impl History {
    pub fn new(pool: Db) -> Self {
        Self { pool }
    }

    async fn events(&self, topic: &Topic, window: Window) -> Result<Vec<Event>, HistoryError> {
        let Some((tenant_id, client, member)) = scope(topic) else { return Ok(Vec::new()) };
        let latest = matches!(window.end, End::Latest);
        let rows: Vec<Row> = Db::query_as(
            "SELECT * FROM (SELECT * FROM messages WHERE tenant_id = $1 AND ($2::uuid IS NULL OR client_id = $2) AND ($3::uuid IS NULL OR member_id = $3)
               AND ($4::uuid IS NULL OR id > $4) AND ($5::timestamptz IS NULL OR created_at > $5)
             ORDER BY CASE WHEN $7 THEN id END DESC, id LIMIT $6) kept ORDER BY id",
        )
        .sqlite(
            "SELECT * FROM (SELECT * FROM messages WHERE tenant_id = $1 AND ($2 IS NULL OR client_id = $2) AND ($3 IS NULL OR member_id = $3)
               AND ($4 IS NULL OR id > $4) AND ($5 IS NULL OR created_at > $5)
             ORDER BY CASE WHEN $7 THEN id END DESC, id LIMIT COALESCE($6, -1)) kept ORDER BY id",
        )
        .bind(tenant_id)
        .bind(client)
        .bind(member)
        .bind(window.after)
        .bind(window.since)
        .bind(window.limit.map(|limit| limit as i64))
        .bind(latest)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(|row| row.delivered().1).collect())
    }
}

/// The tenant, client and member of a messaging topic; `*` is any.
pub(super) fn scope(topic: &Topic) -> Option<(Uuid, Option<Uuid>, Option<Uuid>)> {
    match topic.split('/').collect::<Vec<_>>()[..] {
        ["tenants", tenant, "clients", client, "members", member] => Some((tenant.parse().ok()?, side(client)?, side(member)?)),
        _ => None,
    }
}

fn side(segment: &str) -> Option<Option<Uuid>> {
    match segment {
        "*" => Some(None),
        id => id.parse().ok().map(Some),
    }
}

impl HistoryProvider for History {
    fn fetch_history<'a>(&'a self, topic: &'a Topic, opts: &'a HistoryOptions) -> HistoryFuture<'a> {
        Box::pin(self.events(topic, Window { after: None, since: opts.since, limit: opts.limit, end: End::Latest }))
    }

    fn fetch_history_since_id<'a>(&'a self, topic: &'a Topic, last_id: &'a str, limit: Option<usize>) -> HistoryFuture<'a> {
        Box::pin(self.events(topic, Window { after: last_id.parse().ok(), since: None, limit, end: End::Oldest }))
    }
}

/// The browser's routes, signed in as a client: `POST /tenants/{tenant_id}/members/{member_id}/messages`, `…/events`
/// and `…/attachments` (a body over `cap` bytes refused), and `GET /tenants/{tenant_id}/attachments/{id}`; `base` is
/// where they are mounted, for the files' `fileUri`. Takes the `Broker` from an `Extension`.
pub fn router(cap: usize, base: &str) -> OpenApiRouter<AppContext> {
    OpenApiRouter::new().routes(routes!(send)).routes(routes!(signal)).merge(super::attachment::routes(cap, base))
}

#[derive(Debug, Deserialize, Validate, ToSchema)]
struct Posted {
    tenant_id: Uuid,
    member_id: Uuid,
    #[schema(value_type = Object)]
    body: Value,
}

#[derive(Debug, Deserialize, Validate, ToSchema)]
struct Signalled {
    tenant_id: Uuid,
    member_id: Uuid,
    signal: Signal,
}

/// Sends `body` from the signed-in client to the member.
#[utoipa::path(
    post,
    path = "/tenants/{tenant_id}/members/{member_id}/messages",
    tag = "messages",
    params(("tenant_id" = Uuid, Path), ("member_id" = Uuid, Path)),
    request_body = Posted,
    responses((status = 200, body = Message)),
)]
async fn send(
    State(ctx): State<AppContext>,
    Extension(broker): Extension<Broker>,
    Extension(token): Extension<AccessToken>,
    Input(posted): Input<Posted>,
) -> Result<Json<Message>, MessagingError> {
    let messaging = Messaging::new(ctx.pool, broker, posted.tenant_id);
    Ok(Json(messaging.store(posted.body, Party::Client(token.sub), Party::Member(posted.member_id)).await?))
}

/// Sends an event, such as typing, from the signed-in client to the member.
#[utoipa::path(
    post,
    path = "/tenants/{tenant_id}/members/{member_id}/events",
    tag = "messages",
    params(("tenant_id" = Uuid, Path), ("member_id" = Uuid, Path)),
    request_body = Signalled,
    responses((status = 204)),
)]
async fn signal(
    State(ctx): State<AppContext>,
    Extension(broker): Extension<Broker>,
    Extension(token): Extension<AccessToken>,
    Input(signalled): Input<Signalled>,
) -> Result<StatusCode, MessagingError> {
    let messaging = Messaging::new(ctx.pool, broker, signalled.tenant_id);
    messaging.send_event(signalled.signal, Party::Client(token.sub), Party::Member(signalled.member_id)).await?;
    Ok(StatusCode::NO_CONTENT)
}
