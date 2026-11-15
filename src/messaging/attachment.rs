//! Attachment: a file of a thread, kept in a `bytea` column, sent along with a message by id.

use axum::Extension;
use axum::Json;
use axum::body::{Body, to_bytes};
use axum::extract::{Path, Query, State};
use axum::http::header::CONTENT_TYPE;
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use uuid::Uuid;
use validator::Validate;

use super::error::MessagingError;
use super::hub::{Messaging, Paged};
use super::{participant, thread};
use crate::Entity;
use crate::auth::AccessToken;
use crate::auth::rest::{AppContext, Input, Page, fetch_page};
use crate::booking::value::Name;

/// What a message lists of each attachment it carries.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, sqlx::FromRow, ToSchema)]
pub struct AttachmentSummary {
    pub id: Uuid,
    #[sqlx(try_from = "String")]
    #[schema(value_type = String, example = "price-list.pdf")]
    pub filename: Name,
    #[schema(example = "application/pdf")]
    pub content_type: String,
    pub size: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, sqlx::FromRow)]
pub struct AttachmentData {
    pub tenant_id: Uuid,
    pub thread_id: Uuid,
    pub message_id: Option<Uuid>,
    #[sqlx(try_from = "String")]
    pub filename: Name,
    pub content_type: String,
    pub size: i64,
    #[serde(with = "crate::bytea")]
    pub content: Vec<u8>,
}

pub type Attachment = Entity<Uuid, AttachmentData>;

/// A file to attach: its name, media type and bytes.
#[derive(Debug, Clone, PartialEq)]
pub struct Upload {
    pub filename: Name,
    pub content_type: String,
    pub content: Vec<u8>,
}

impl Messaging {
    /// Stores `upload` in the thread, sent by participant `sender`; over the attachment limit is `TooLarge`.
    pub async fn attach(&self, tenant_id: Uuid, thread_id: Uuid, sender: Uuid, upload: Upload) -> Result<Attachment, MessagingError> {
        let limit = self.attachment_limit();
        (upload.content.len() <= limit).then_some(()).ok_or(MessagingError::TooLarge { limit })?;
        participant::require(&self.pool, tenant_id, thread_id, sender).await?;
        Ok(sqlx::query_as(
            "INSERT INTO attachments (tenant_id, id, thread_id, filename, content_type, size, content)
             VALUES ($1, $2, $3, $4, $5, $6, $7) RETURNING *",
        )
        .bind(tenant_id)
        .bind(Uuid::now_v7())
        .bind(thread_id)
        .bind(&upload.filename)
        .bind(&upload.content_type)
        .bind(upload.content.len() as i64)
        .bind(&upload.content)
        .fetch_one(&self.pool)
        .await?)
    }

    pub async fn attachment(&self, tenant_id: Uuid, id: Uuid) -> Result<Attachment, MessagingError> {
        sqlx::query_as("SELECT * FROM attachments WHERE tenant_id = $1 AND id = $2 AND deleted_at IS NULL")
            .bind(tenant_id)
            .bind(id)
            .fetch_optional(&self.pool)
            .await?
            .ok_or(MessagingError::NotFound)
    }
}

pub fn routes() -> OpenApiRouter<AppContext> {
    OpenApiRouter::new().routes(routes!(list, upload)).routes(routes!(download))
}

#[derive(Debug, Deserialize)]
struct Named {
    filename: Name,
}

#[derive(Debug, Deserialize, Validate)]
struct Download {
    tenant_id: Uuid,
    id: Uuid,
    aid: Uuid,
}

#[utoipa::path(
    get,
    path = "/tenants/{tenant_id}/threads/{id}/attachments",
    tag = "attachments",
    params(("tenant_id" = Uuid, Path), ("id" = Uuid, Path), ("page_size" = Option<u16>, Query), ("page_token" = Option<String>, Query)),
    responses((status = 200, body = inline(Page<AttachmentSummary>)), (status = 404)),
)]
async fn list(State(ctx): State<AppContext>, Input(paged): Input<Paged>) -> Result<Page<AttachmentSummary>, MessagingError> {
    thread::require(&ctx.pool, paged.tenant_id, paged.id).await?;
    let mut query = sqlx::QueryBuilder::new("SELECT id, filename, content_type, size FROM attachments WHERE deleted_at IS NULL AND tenant_id = ");
    query.push_bind(paged.tenant_id).push(" AND thread_id = ").push_bind(paged.id);
    Ok(fetch_page(&ctx.pool, query, &paged.page).await?)
}

/// The raw body is the file; its `Content-Type` is kept and served back.
#[utoipa::path(
    post,
    path = "/tenants/{tenant_id}/threads/{id}/attachments",
    tag = "attachments",
    params(("tenant_id" = Uuid, Path), ("id" = Uuid, Path), ("filename" = String, Query)),
    request_body(content = Vec<u8>, content_type = "application/octet-stream"),
    responses((status = 200, body = AttachmentSummary), (status = 404), (status = 413)),
)]
async fn upload(
    Extension(messaging): Extension<Messaging>,
    Extension(token): Extension<AccessToken>,
    Path((tenant_id, thread_id)): Path<(Uuid, Uuid)>,
    Query(named): Query<Named>,
    headers: HeaderMap,
    body: Body,
) -> Result<Json<AttachmentSummary>, MessagingError> {
    let limit = messaging.attachment_limit();
    let content = to_bytes(body, limit.saturating_add(1)).await.map_err(|_| MessagingError::TooLarge { limit })?;
    let content_type = headers.get(CONTENT_TYPE).and_then(|value| value.to_str().ok()).unwrap_or("application/octet-stream");
    let sender = messaging.speaker(tenant_id, thread_id, token.sub).await?;
    let upload = Upload { filename: named.filename, content_type: content_type.into(), content: content.to_vec() };
    let stored = messaging.attach(tenant_id, thread_id, sender.id, upload).await?;
    Ok(Json(AttachmentSummary { id: stored.id, filename: stored.filename.clone(), content_type: stored.content_type.clone(), size: stored.size }))
}

#[utoipa::path(
    get,
    path = "/tenants/{tenant_id}/threads/{id}/attachments/{aid}",
    tag = "attachments",
    params(("tenant_id" = Uuid, Path), ("id" = Uuid, Path), ("aid" = Uuid, Path)),
    responses((status = 200, content_type = "application/octet-stream", body = Vec<u8>), (status = 404)),
)]
async fn download(Extension(messaging): Extension<Messaging>, Input(key): Input<Download>) -> Result<Response, MessagingError> {
    let stored = messaging.attachment(key.tenant_id, key.aid).await?;
    let stored = (stored.thread_id == key.id).then_some(stored).ok_or(MessagingError::NotFound)?;
    Ok(([(CONTENT_TYPE, stored.content_type.clone())], stored.content.clone()).into_response())
}
