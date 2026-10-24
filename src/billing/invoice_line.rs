//! Invoice line: one product or service billed. Changed only while its invoice is a draft.

use axum::extract::State;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use uuid::Uuid;
use validator::{Validate, ValidationError};

use super::draft::{ensure_draft, ensure_line_draft, refuse_changes, Line};
use super::invoice::InvoiceData;
use super::value::{Micros, Percent, Quantity};
use crate::auth::crud;
use crate::auth::rest::{ApiError, ApiResult, AppContext, Changes, Input, Key, Keyed, ListRequest, Page};
use crate::booking::value::Name;
use crate::{Entity, Table};

/// What is billed: a product, a service (with the appointment it was for), or neither for a free line.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Validate, sqlx::FromRow, Table, ToSchema)]
#[table(name = "billing_invoice_lines", keys = ["org_id", "id"])]
#[validate(schema(function = "names_one_target"))]
pub struct InvoiceLineData {
    pub org_id: Uuid,
    pub invoice_id: Uuid,
    pub position: i32,
    pub product_id: Option<Uuid>,
    pub service_id: Option<Uuid>,
    pub appointment_id: Option<Uuid>,
    #[schema(value_type = String, example = "Tooth cleaning")]
    pub description: Name,
    pub quantity: Quantity,
    pub unit_price: Micros,
    #[serde(default)]
    pub discount: Percent,
    #[serde(default)]
    pub tax_ids: Vec<Uuid>,
    #[serde(default)]
    pub sub_total: Micros,
    #[serde(default)]
    pub total: Micros,
}

pub type InvoiceLine = Entity<Uuid, InvoiceLineData>;

impl Line for InvoiceLineData {
    type Parent = InvoiceData;
    const PARENT_ID: &'static str = "invoice_id";
}

fn names_one_target(line: &InvoiceLineData) -> Result<(), ValidationError> {
    (line.product_id.is_none() || line.service_id.is_none())
        .then_some(())
        .ok_or_else(|| ValidationError::new("a line names a product or a service, not both"))
}

/// The line's invoice must be an invoice of the org (a bad request if not) and still a draft (a conflict if not).
async fn in_draft_invoice(pool: &PgPool, line: &InvoiceLineData) -> ApiResult<()> {
    match ensure_draft::<InvoiceData>(pool, line.org_id, line.invoice_id).await {
        Err(ApiError::NotFound) => Err(ApiError::BadRequest("invoice_id is not an invoice of the org".into())),
        other => other,
    }
}

pub fn routes() -> OpenApiRouter<AppContext> {
    OpenApiRouter::new()
        .routes(routes!(list, create))
        .routes(routes!(get, replace, patch, delete))
}

#[utoipa::path(
    get,
    path = "/orgs/{org_id}/invoiceLines",
    tag = "invoiceLines",
    params(
        ("org_id" = Uuid, Path),
        ("filter" = Option<String>, Query, description = "AIP-160 filter (https://google.aip.dev/160) on any field of the entity; other names are looked up in `attributes`."),
        ("page_size" = Option<u16>, Query),
        ("page_token" = Option<String>, Query),
    ),
    responses((status = 200, body = inline(Page<Entity<Uuid, InvoiceLineData>>))),
)]
async fn list(State(ctx): State<AppContext>, Input(request): Input<ListRequest>) -> ApiResult<Page<InvoiceLine>> {
    crud::list(&ctx.pool, &request).await
}

#[utoipa::path(
    get,
    path = "/orgs/{org_id}/invoiceLines/{id}",
    tag = "invoiceLines",
    params(("org_id" = Uuid, Path), ("id" = Uuid, Path)),
    responses((status = 200, body = inline(Entity<Uuid, InvoiceLineData>)), (status = 404)),
)]
async fn get(State(ctx): State<AppContext>, Input(key): Input<Key>) -> ApiResult<InvoiceLine> {
    crud::get(&ctx.pool, &key).await
}

#[utoipa::path(
    post,
    path = "/orgs/{org_id}/invoiceLines",
    tag = "invoiceLines",
    params(("org_id" = Uuid, Path)),
    request_body = InvoiceLineData,
    responses((status = 200, body = inline(Entity<Uuid, InvoiceLineData>)), (status = 400), (status = 409)),
)]
async fn create(State(ctx): State<AppContext>, Input(data): Input<InvoiceLineData>) -> ApiResult<InvoiceLine> {
    in_draft_invoice(&ctx.pool, &data).await?;
    crud::create(&ctx.pool, data).await
}

#[utoipa::path(
    put,
    path = "/orgs/{org_id}/invoiceLines/{id}",
    tag = "invoiceLines",
    params(("org_id" = Uuid, Path), ("id" = Uuid, Path)),
    request_body = InvoiceLineData,
    responses((status = 200, body = inline(Entity<Uuid, InvoiceLineData>)), (status = 400), (status = 404), (status = 409)),
)]
async fn replace(State(ctx): State<AppContext>, Input(body): Input<Keyed<InvoiceLineData>>) -> ApiResult<InvoiceLine> {
    ensure_line_draft::<InvoiceLineData>(&ctx.pool, body.data.org_id, body.id).await?;
    in_draft_invoice(&ctx.pool, &body.data).await?;
    crud::replace(&ctx.pool, body).await
}

/// Change some fields (JSON merge patch, RFC 7396): fields given are changed, fields set to null are reset.
#[utoipa::path(
    patch,
    path = "/orgs/{org_id}/invoiceLines/{id}",
    tag = "invoiceLines",
    params(("org_id" = Uuid, Path), ("id" = Uuid, Path)),
    request_body = Object,
    responses((status = 200, body = inline(Entity<Uuid, InvoiceLineData>)), (status = 400), (status = 404), (status = 409)),
)]
async fn patch(State(ctx): State<AppContext>, Input(body): Input<Changes>) -> ApiResult<InvoiceLine> {
    ensure_line_draft::<InvoiceLineData>(&ctx.pool, body.key.org_id, body.key.id).await?;
    refuse_changes(&body, &["invoice_id"])?;
    crud::patch(&ctx.pool, body).await
}

#[utoipa::path(
    delete,
    path = "/orgs/{org_id}/invoiceLines/{id}",
    tag = "invoiceLines",
    params(("org_id" = Uuid, Path), ("id" = Uuid, Path)),
    responses((status = 200, body = inline(Entity<Uuid, InvoiceLineData>)), (status = 404), (status = 409)),
)]
async fn delete(State(ctx): State<AppContext>, Input(key): Input<Key>) -> ApiResult<InvoiceLine> {
    ensure_line_draft::<InvoiceLineData>(&ctx.pool, key.org_id, key.id).await?;
    crud::delete(&ctx.pool, &key).await
}
