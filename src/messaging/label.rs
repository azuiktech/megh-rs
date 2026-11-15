//! Label: a tenant's tag, put on threads and on messages.

use axum::Extension;
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::put;
use serde::{Deserialize, Serialize};
use serde_json::json;
use utoipa::{OpenApi, ToSchema};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use uuid::Uuid;
use validator::Validate;

use super::error::MessagingError;
use super::hub::{Messaging, thread_topic};
use super::thread;
use crate::auth::crud;
use crate::auth::rest::{ApiResult, AppContext, Changes, Input, Key, Keyed, ListRequest, Page};
use crate::booking::value::Name;
use crate::event::Event;
use crate::{Entity, Table};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Validate, sqlx::FromRow, Table, ToSchema)]
#[table(name = "labels", keys = ["tenant_id", "id"])]
#[schema(example = json!({"name": "urgent", "colour": "#ff0000"}))]
pub struct LabelData {
    pub tenant_id: Uuid,
    #[sqlx(try_from = "String")]
    #[schema(value_type = String)]
    pub name: Name,
    #[serde(default)]
    pub colour: Option<String>,
}

pub type Label = Entity<Uuid, LabelData>;

/// What a label is put on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Labelled {
    Thread(Uuid),
    Message(Uuid),
}

impl Labelled {
    /// The table that holds its labels, its column there, and its id.
    fn links(self) -> (&'static str, &'static str, Uuid) {
        match self {
            Self::Thread(id) => ("thread_labels", "thread_id", id),
            Self::Message(id) => ("message_labels", "message_id", id),
        }
    }
}

impl Messaging {
    /// Puts the label on; putting it on twice is the same as once. Published as the labels the target now has.
    pub async fn label(&self, tenant_id: Uuid, target: Labelled, label_id: Uuid) -> Result<(), MessagingError> {
        let (table, column, id) = target.links();
        let thread_id = self.thread_of(tenant_id, target).await?;
        sqlx::query(&format!("INSERT INTO {table} (tenant_id, {column}, label_id) VALUES ($1, $2, $3) ON CONFLICT DO NOTHING"))
            .bind(tenant_id)
            .bind(id)
            .bind(label_id)
            .execute(&self.pool)
            .await?;
        self.announce_labels(tenant_id, thread_id, target).await
    }

    pub async fn unlabel(&self, tenant_id: Uuid, target: Labelled, label_id: Uuid) -> Result<(), MessagingError> {
        let (table, column, id) = target.links();
        let thread_id = self.thread_of(tenant_id, target).await?;
        sqlx::query(&format!("DELETE FROM {table} WHERE tenant_id = $1 AND {column} = $2 AND label_id = $3"))
            .bind(tenant_id)
            .bind(id)
            .bind(label_id)
            .execute(&self.pool)
            .await?;
        self.announce_labels(tenant_id, thread_id, target).await
    }

    /// `NotFound` unless the message belongs to the thread.
    async fn in_thread(&self, tenant_id: Uuid, thread_id: Uuid, message_id: Uuid) -> Result<(), MessagingError> {
        (self.thread_of(tenant_id, Labelled::Message(message_id)).await? == thread_id).then_some(()).ok_or(MessagingError::NotFound)
    }

    async fn thread_of(&self, tenant_id: Uuid, target: Labelled) -> Result<Uuid, MessagingError> {
        match target {
            Labelled::Thread(id) => thread::require(&self.pool, tenant_id, id).await.map(|()| id),
            Labelled::Message(id) => sqlx::query_scalar("SELECT thread_id FROM messages WHERE tenant_id = $1 AND id = $2 AND deleted_at IS NULL")
                .bind(tenant_id)
                .bind(id)
                .fetch_optional(&self.pool)
                .await?
                .ok_or(MessagingError::NotFound),
        }
    }

    async fn announce_labels(&self, tenant_id: Uuid, thread_id: Uuid, target: Labelled) -> Result<(), MessagingError> {
        let (table, column, id) = target.links();
        let labels: Vec<Uuid> = sqlx::query_scalar(&format!(
            "SELECT label_id FROM {table} WHERE tenant_id = $1 AND {column} = $2 AND deleted_at IS NULL ORDER BY label_id"
        ))
        .bind(tenant_id)
        .bind(id)
        .fetch_all(&self.pool)
        .await?;
        self.announce(Event::new(thread_topic(tenant_id, thread_id), json!({"kind": "labels", "target": target, "labels": labels}))).await;
        Ok(())
    }
}

#[derive(OpenApi)]
#[openapi(paths(label_thread, unlabel_thread, label_message, unlabel_message))]
struct Putting;

pub fn routes() -> OpenApiRouter<AppContext> {
    OpenApiRouter::with_openapi(Putting::openapi())
        .routes(routes!(list, create))
        .routes(routes!(get, replace, patch, delete))
        .route("/tenants/{tenant_id}/threads/{id}/labels/{label_id}", put(label_thread).delete(unlabel_thread))
        .route("/tenants/{tenant_id}/threads/{id}/messages/{mid}/labels/{label_id}", put(label_message).delete(unlabel_message))
}

#[derive(Debug, Deserialize, Validate)]
struct OnThread {
    tenant_id: Uuid,
    id: Uuid,
    label_id: Uuid,
}

#[derive(Debug, Deserialize, Validate)]
struct OnMessage {
    tenant_id: Uuid,
    id: Uuid,
    mid: Uuid,
    label_id: Uuid,
}

#[utoipa::path(
    get,
    path = "/tenants/{tenant_id}/labels",
    tag = "labels",
    params(
        ("tenant_id" = Uuid, Path),
        ("filter" = Option<String>, Query, description = "AIP-160 filter (https://google.aip.dev/160) on any field of the entity."),
        ("page_size" = Option<u16>, Query),
        ("page_token" = Option<String>, Query),
    ),
    responses((status = 200, body = inline(Page<Entity<Uuid, LabelData>>))),
)]
async fn list(State(ctx): State<AppContext>, Input(request): Input<ListRequest>) -> ApiResult<Page<Label>> {
    crud::list(&ctx.pool, &request).await
}

#[utoipa::path(
    get,
    path = "/tenants/{tenant_id}/labels/{id}",
    tag = "labels",
    params(("tenant_id" = Uuid, Path), ("id" = Uuid, Path)),
    responses((status = 200, body = inline(Entity<Uuid, LabelData>)), (status = 404)),
)]
async fn get(State(ctx): State<AppContext>, Input(key): Input<Key>) -> ApiResult<Label> {
    crud::get(&ctx.pool, &key).await
}

#[utoipa::path(
    post,
    path = "/tenants/{tenant_id}/labels",
    tag = "labels",
    params(("tenant_id" = Uuid, Path)),
    request_body = LabelData,
    responses((status = 200, body = inline(Entity<Uuid, LabelData>))),
)]
async fn create(State(ctx): State<AppContext>, Input(data): Input<LabelData>) -> ApiResult<Label> {
    crud::create(&ctx.pool, data).await
}

#[utoipa::path(
    put,
    path = "/tenants/{tenant_id}/labels/{id}",
    tag = "labels",
    params(("tenant_id" = Uuid, Path), ("id" = Uuid, Path)),
    request_body = LabelData,
    responses((status = 200, body = inline(Entity<Uuid, LabelData>)), (status = 404)),
)]
async fn replace(State(ctx): State<AppContext>, Input(body): Input<Keyed<LabelData>>) -> ApiResult<Label> {
    crud::replace(&ctx.pool, body).await
}

/// Change some fields (JSON merge patch, RFC 7396): fields given are changed, fields set to null are reset.
#[utoipa::path(
    patch,
    path = "/tenants/{tenant_id}/labels/{id}",
    tag = "labels",
    params(("tenant_id" = Uuid, Path), ("id" = Uuid, Path)),
    request_body(content = LabelData, description = "Only the fields to change; null resets a field to its default (JSON merge patch, RFC 7396)."),
    responses((status = 200, body = inline(Entity<Uuid, LabelData>)), (status = 404)),
)]
async fn patch(State(ctx): State<AppContext>, Input(body): Input<Changes>) -> ApiResult<Label> {
    crud::patch(&ctx.pool, body).await
}

#[utoipa::path(
    delete,
    path = "/tenants/{tenant_id}/labels/{id}",
    tag = "labels",
    params(("tenant_id" = Uuid, Path), ("id" = Uuid, Path)),
    responses((status = 200, body = inline(Entity<Uuid, LabelData>)), (status = 404)),
)]
async fn delete(State(ctx): State<AppContext>, Input(key): Input<Key>) -> ApiResult<Label> {
    crud::delete(&ctx.pool, &key).await
}

#[utoipa::path(
    put,
    path = "/tenants/{tenant_id}/threads/{id}/labels/{label_id}",
    tag = "labels",
    params(("tenant_id" = Uuid, Path), ("id" = Uuid, Path), ("label_id" = Uuid, Path)),
    responses((status = 204), (status = 400), (status = 404)),
)]
async fn label_thread(Extension(messaging): Extension<Messaging>, Input(on): Input<OnThread>) -> Result<StatusCode, MessagingError> {
    messaging.label(on.tenant_id, Labelled::Thread(on.id), on.label_id).await.map(|()| StatusCode::NO_CONTENT)
}

#[utoipa::path(
    delete,
    path = "/tenants/{tenant_id}/threads/{id}/labels/{label_id}",
    tag = "labels",
    params(("tenant_id" = Uuid, Path), ("id" = Uuid, Path), ("label_id" = Uuid, Path)),
    responses((status = 204), (status = 404)),
)]
async fn unlabel_thread(Extension(messaging): Extension<Messaging>, Input(on): Input<OnThread>) -> Result<StatusCode, MessagingError> {
    messaging.unlabel(on.tenant_id, Labelled::Thread(on.id), on.label_id).await.map(|()| StatusCode::NO_CONTENT)
}

#[utoipa::path(
    put,
    path = "/tenants/{tenant_id}/threads/{id}/messages/{mid}/labels/{label_id}",
    tag = "labels",
    params(("tenant_id" = Uuid, Path), ("id" = Uuid, Path), ("mid" = Uuid, Path), ("label_id" = Uuid, Path)),
    responses((status = 204), (status = 400), (status = 404)),
)]
async fn label_message(Extension(messaging): Extension<Messaging>, Input(on): Input<OnMessage>) -> Result<StatusCode, MessagingError> {
    messaging.in_thread(on.tenant_id, on.id, on.mid).await?;
    messaging.label(on.tenant_id, Labelled::Message(on.mid), on.label_id).await.map(|()| StatusCode::NO_CONTENT)
}

#[utoipa::path(
    delete,
    path = "/tenants/{tenant_id}/threads/{id}/messages/{mid}/labels/{label_id}",
    tag = "labels",
    params(("tenant_id" = Uuid, Path), ("id" = Uuid, Path), ("mid" = Uuid, Path), ("label_id" = Uuid, Path)),
    responses((status = 204), (status = 404)),
)]
async fn unlabel_message(Extension(messaging): Extension<Messaging>, Input(on): Input<OnMessage>) -> Result<StatusCode, MessagingError> {
    messaging.in_thread(on.tenant_id, on.id, on.mid).await?;
    messaging.unlabel(on.tenant_id, Labelled::Message(on.mid), on.label_id).await.map(|()| StatusCode::NO_CONTENT)
}
