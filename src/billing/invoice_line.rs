//! Invoice line: one product or service billed. Changed only while its invoice is a draft.

use axum::extract::State;
use serde::{Deserialize, Serialize};
use crate::Tx;
use utoipa::ToSchema;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use uuid::Uuid;

use crate::Array;
use validator::{Validate, ValidationError};

use super::draft::{ensure_draft, ensure_line_draft, refuse_changes, Line, Lock};
use super::invoice::Invoice;
use super::value::{Micros, Percent, Quantity};
use crate::auth::crud;
use crate::auth::rest::{ApiError, ApiResult, AppContext, Changes, Input, Key, ListRequest, Page};
use crate::booking::value::Name;

/// What is billed: a product, a service (with the appointment it was for), or neither for a free line.
#[crate::entity(table = "billing_invoice_lines", keys = ["tenant_id", "id"], timestamps, soft_delete)]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Validate, sqlx::FromRow, ToSchema)]
#[validate(schema(function = "names_one_target"))]
pub struct InvoiceLine {
    pub tenant_id: Uuid,
    #[serde(default = "Uuid::now_v7")]
    pub id: Uuid,
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
    #[sqlx(try_from = "Array<Uuid>")]
    pub tax_ids: Vec<Uuid>,
    #[serde(default)]
    pub sub_total: Micros,
    #[serde(default)]
    pub total: Micros,
}

impl Line for InvoiceLine {
    type Parent = Invoice;
    const PARENT_ID: &'static str = "invoice_id";
}

fn names_one_target(line: &InvoiceLine) -> Result<(), ValidationError> {
    (line.product_id.is_none() || line.service_id.is_none())
        .then_some(())
        .ok_or_else(|| ValidationError::new("a line names a product or a service, not both"))
}

/// The line's invoice must be an invoice of the tenant (a bad request if not) and still a draft (a conflict if not).
async fn in_draft_invoice(connection: &mut Tx, line: &InvoiceLine) -> ApiResult<()> {
    match ensure_draft::<Invoice>(connection, line.tenant_id, line.invoice_id, Lock::Share).await {
        Err(ApiError::NotFound) => Err(ApiError::BadRequest("invoice_id is not an invoice of the tenant".into())),
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
    path = "/tenants/{tenant_id}/invoiceLines",
    tag = "invoiceLines",
    params(
        ("tenant_id" = Uuid, Path),
        ("filter" = Option<String>, Query, description = "AIP-160 filter (https://google.aip.dev/160) on any field of the entity; other names are looked up in `attributes`."),
        ("page_size" = Option<u16>, Query),
        ("page_token" = Option<String>, Query),
    ),
    responses((status = 200, body = inline(Page<InvoiceLine>))),
)]
async fn list(State(ctx): State<AppContext>, Input(request): Input<ListRequest>) -> ApiResult<Page<InvoiceLine>> {
    crud::list(&ctx.pool, &request).await
}

#[utoipa::path(
    get,
    path = "/tenants/{tenant_id}/invoiceLines/{id}",
    tag = "invoiceLines",
    params(("tenant_id" = Uuid, Path), ("id" = Uuid, Path)),
    responses((status = 200, body = inline(InvoiceLine)), (status = 404)),
)]
async fn get(State(ctx): State<AppContext>, Input(key): Input<Key>) -> ApiResult<InvoiceLine> {
    crud::get(&ctx.pool, &key).await
}

#[utoipa::path(
    post,
    path = "/tenants/{tenant_id}/invoiceLines",
    tag = "invoiceLines",
    params(("tenant_id" = Uuid, Path)),
    request_body = InvoiceLine,
    responses((status = 200, body = inline(InvoiceLine)), (status = 400), (status = 409)),
)]
async fn create(State(ctx): State<AppContext>, Input(data): Input<InvoiceLine>) -> ApiResult<InvoiceLine> {
    let mut transaction = ctx.pool.begin().await?;
    in_draft_invoice(&mut transaction, &data).await?;
    let created = crud::create(&mut transaction, data, None).await?;
    transaction.commit().await?;
    Ok(created)
}

#[utoipa::path(
    put,
    path = "/tenants/{tenant_id}/invoiceLines/{id}",
    tag = "invoiceLines",
    params(("tenant_id" = Uuid, Path), ("id" = Uuid, Path)),
    request_body = InvoiceLine,
    responses((status = 200, body = inline(InvoiceLine)), (status = 400), (status = 404), (status = 409)),
)]
async fn replace(State(ctx): State<AppContext>, Input(body): Input<InvoiceLine>) -> ApiResult<InvoiceLine> {
    let mut transaction = ctx.pool.begin().await?;
    ensure_line_draft::<InvoiceLine>(&mut transaction, body.tenant_id, body.id).await?;
    let current = crud::get::<InvoiceLine>(&mut transaction, &Key { tenant_id: body.tenant_id, id: body.id }).await?;
    (current.invoice_id == body.invoice_id)
        .then_some(())
        .ok_or_else(|| ApiError::BadRequest("invoice_id cannot be changed".into()))?;
    let replaced = crud::replace(&mut transaction, body, None).await?;
    transaction.commit().await?;
    Ok(replaced)
}

/// Change some fields (JSON merge patch, RFC 7396): fields given are changed, fields set to null are reset.
#[utoipa::path(
    patch,
    path = "/tenants/{tenant_id}/invoiceLines/{id}",
    tag = "invoiceLines",
    params(("tenant_id" = Uuid, Path), ("id" = Uuid, Path)),
    request_body = Object,
    responses((status = 200, body = inline(InvoiceLine)), (status = 400), (status = 404), (status = 409)),
)]
async fn patch(State(ctx): State<AppContext>, Input(body): Input<Changes>) -> ApiResult<InvoiceLine> {
    let mut transaction = ctx.pool.begin().await?;
    ensure_line_draft::<InvoiceLine>(&mut transaction, body.key.tenant_id, body.key.id).await?;
    refuse_changes(&body, &["invoice_id"])?;
    let patched = crud::patch(&mut transaction, body, None).await?;
    transaction.commit().await?;
    Ok(patched)
}

#[utoipa::path(
    delete,
    path = "/tenants/{tenant_id}/invoiceLines/{id}",
    tag = "invoiceLines",
    params(("tenant_id" = Uuid, Path), ("id" = Uuid, Path)),
    responses((status = 200, body = inline(InvoiceLine)), (status = 404), (status = 409)),
)]
async fn delete(State(ctx): State<AppContext>, Input(key): Input<Key>) -> ApiResult<InvoiceLine> {
    let mut transaction = ctx.pool.begin().await?;
    ensure_line_draft::<InvoiceLine>(&mut transaction, key.tenant_id, key.id).await?;
    let deleted = crud::delete(&mut transaction, &key, None).await?;
    transaction.commit().await?;
    Ok(deleted)
}

