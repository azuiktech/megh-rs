//! Application: part of a payment, or of a credit note or vendor credit, set against an invoice or bill.

use axum::extract::State;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::auth::crud;
use crate::auth::rest::{ApiError, ApiResult, AppContext, Changes, Input, Key, Keyed, ListRequest, Page};
use crate::billing::draft::refuse_changes;
use crate::billing::invoice::{InvoiceKind, InvoiceStatus};
use crate::billing::payment::{Application, ApplicationData, PaymentKind};
use uuid::Uuid;
use crate::Entity;
use sqlx::PgPool;

/// What an application must be: of an open invoice or bill (a conflict if not), of a payment or credit of the
/// same party, whose kind fits it (a bad request if not).
async fn check(pool: &PgPool, application: &ApplicationData) -> ApiResult<()> {
    let org_id = application.org_id;
    let invoice = document(pool, org_id, application.invoice_id).await?.ok_or_else(|| bad("invoice_id is not an invoice or bill of the org"))?;
    open(&invoice)?;
    match (application.payment_id, application.credit_id) {
        (Some(payment), _) => check_payment(pool, org_id, &invoice, payment).await,
        (None, Some(credit)) => check_credit(pool, org_id, &invoice, credit).await,
        (None, None) => Err(bad("an application is of a payment or a credit")),
    }
}

/// An invoice, bill, credit note or vendor credit, as far as an application needs.
#[derive(sqlx::FromRow)]
struct Document {
    kind: InvoiceKind,
    party_id: Uuid,
    status: InvoiceStatus,
}

#[derive(sqlx::FromRow)]
struct Paid {
    kind: PaymentKind,
    party_id: Uuid,
}

fn bad(message: &str) -> ApiError {
    ApiError::BadRequest(message.into())
}

fn open(document: &Document) -> ApiResult<()> {
    (document.status == InvoiceStatus::Open)
        .then_some(())
        .ok_or_else(|| ApiError::Conflict("the invoice is not open".into()))
}

async fn document(pool: &PgPool, org_id: Uuid, id: Uuid) -> ApiResult<Option<Document>> {
    Ok(sqlx::query_as("SELECT kind, party_id, status FROM billing_invoices WHERE org_id = $1 AND id = $2 AND deleted_at IS NULL")
        .bind(org_id)
        .bind(id)
        .fetch_optional(pool)
        .await?)
}

async fn check_payment(pool: &PgPool, org_id: Uuid, invoice: &Document, id: Uuid) -> ApiResult<()> {
    let payment: Paid = sqlx::query_as("SELECT kind, party_id FROM billing_payments WHERE org_id = $1 AND id = $2 AND deleted_at IS NULL")
        .bind(org_id)
        .bind(id)
        .fetch_optional(pool)
        .await?
        .ok_or_else(|| bad("payment_id is not a payment of the org"))?;
    (invoice.kind.settled_by() == Some(payment.kind)).then_some(()).ok_or_else(|| bad("the payment goes the wrong way for this document"))?;
    (payment.party_id == invoice.party_id).then_some(()).ok_or_else(|| bad("the payment is of another party"))
}

async fn check_credit(pool: &PgPool, org_id: Uuid, invoice: &Document, id: Uuid) -> ApiResult<()> {
    let credit = document(pool, org_id, id).await?.ok_or_else(|| bad("credit_id is not a credit note or vendor credit of the org"))?;
    (credit.kind.credits() == Some(invoice.kind)).then_some(()).ok_or_else(|| bad("the credit does not credit this kind of document"))?;
    (credit.party_id == invoice.party_id).then_some(()).ok_or_else(|| bad("the credit is of another party"))?;
    open(&credit)
}

pub fn routes() -> OpenApiRouter<AppContext> {
    OpenApiRouter::new()
        .routes(routes!(list, create))
        .routes(routes!(get, replace, patch, delete))
}

#[utoipa::path(
    get,
    path = "/orgs/{org_id}/applications",
    tag = "applications",
    params(
        ("org_id" = Uuid, Path),
        ("filter" = Option<String>, Query, description = "AIP-160 filter (https://google.aip.dev/160) on any field of the entity; other names are looked up in `attributes`."),
        ("page_size" = Option<u16>, Query),
        ("page_token" = Option<String>, Query),
    ),
    responses((status = 200, body = inline(Page<Entity<Uuid, ApplicationData>>))),
)]
async fn list(State(ctx): State<AppContext>, Input(request): Input<ListRequest>) -> ApiResult<Page<Application>> {
    crud::list(&ctx.pool, &request).await
}

#[utoipa::path(
    get,
    path = "/orgs/{org_id}/applications/{id}",
    tag = "applications",
    params(("org_id" = Uuid, Path), ("id" = Uuid, Path)),
    responses((status = 200, body = inline(Entity<Uuid, ApplicationData>)), (status = 404)),
)]
async fn get(State(ctx): State<AppContext>, Input(key): Input<Key>) -> ApiResult<Application> {
    crud::get(&ctx.pool, &key).await
}

#[utoipa::path(
    post,
    path = "/orgs/{org_id}/applications",
    tag = "applications",
    params(("org_id" = Uuid, Path)),
    request_body = ApplicationData,
    responses((status = 200, body = inline(Entity<Uuid, ApplicationData>)), (status = 400)),
)]
async fn create(State(ctx): State<AppContext>, Input(data): Input<ApplicationData>) -> ApiResult<Application> {
    check(&ctx.pool, &data).await?;
    crud::create(&ctx.pool, data).await
}

#[utoipa::path(
    put,
    path = "/orgs/{org_id}/applications/{id}",
    tag = "applications",
    params(("org_id" = Uuid, Path), ("id" = Uuid, Path)),
    request_body = ApplicationData,
    responses((status = 200, body = inline(Entity<Uuid, ApplicationData>)), (status = 400), (status = 404), (status = 409)),
)]
async fn replace(State(ctx): State<AppContext>, Input(body): Input<Keyed<ApplicationData>>) -> ApiResult<Application> {
    check(&ctx.pool, &body.data).await?;
    crud::replace(&ctx.pool, body).await
}

/// Change some fields (JSON merge patch, RFC 7396): fields given are changed, fields set to null are reset.
#[utoipa::path(
    patch,
    path = "/orgs/{org_id}/applications/{id}",
    tag = "applications",
    params(("org_id" = Uuid, Path), ("id" = Uuid, Path)),
    request_body = Object,
    responses((status = 200, body = inline(Entity<Uuid, ApplicationData>)), (status = 400), (status = 404), (status = 409)),
)]
async fn patch(State(ctx): State<AppContext>, Input(body): Input<Changes>) -> ApiResult<Application> {
    refuse_changes(&body, &["invoice_id", "payment_id", "credit_id"])?;
    crud::patch(&ctx.pool, body).await
}

#[utoipa::path(
    delete,
    path = "/orgs/{org_id}/applications/{id}",
    tag = "applications",
    params(("org_id" = Uuid, Path), ("id" = Uuid, Path)),
    responses((status = 200, body = inline(Entity<Uuid, ApplicationData>)), (status = 404), (status = 409)),
)]
async fn delete(State(ctx): State<AppContext>, Input(key): Input<Key>) -> ApiResult<Application> {
    crud::delete(&ctx.pool, &key).await
}
