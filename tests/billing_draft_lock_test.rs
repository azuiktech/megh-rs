//! Issue #211 (#4 of the code review): a change to a draft invoice or its lines and `:open` wait for each other, so a
//! change that arrives while the invoice is being opened is refused once it is open, instead of landing on it.

#[macro_use]
mod common;

use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::middleware::from_fn;
use axum::{Extension, Router};
use megh::auth::rest::AppContext;
use megh::{authorizer, Grant};
use serde_json::{json, Value};
use megh::{Db, Tx};
use tower::ServiceExt;
use uuid::Uuid;

async fn tenant(pool: &Db, name: &str) -> Uuid {
    Db::query_scalar("INSERT INTO tenants (name) VALUES ($1) RETURNING id").bind(name).fetch_one(pool).await.unwrap()
}

fn app(pool: Db, tenant: Uuid) -> Router {
    let (api, _) = megh::billing::router().split_for_parts();
    let grants = [format!("tenants:read:{tenant}"), "invoices:*".into(), "invoiceLines:*".into()];
    Router::new()
        .nest("/api/v1", api.layer(from_fn(authorizer)))
        .layer(Extension(grants.iter().map(Grant::new).collect::<Vec<_>>()))
        .with_state(AppContext { pool })
}

async fn call(app: Router, method: &'static str, uri: String, body: Value) -> (StatusCode, Value) {
    let request = Request::builder().method(method).uri(uri);
    let request = match body {
        Value::Null => request.body(Body::empty()),
        body => request.header("content-type", "application/json").body(Body::from(body.to_string())),
    };
    let response = app.oneshot(request.unwrap()).await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

fn invoice(customer: Uuid) -> Value {
    json!({"party_kind": "tenant", "party_id": customer, "status": "draft", "date": "2026-10-22", "due_date": "2026-11-21", "currency": "USD"})
}

fn line(invoice: &str) -> Value {
    json!({"invoice_id": invoice, "position": 1, "description": "Tooth cleaning", "quantity": "1", "unit_price": 80000000})
}

/// A draft invoice with one line: the invoice id, the line id and the app.
async fn draft(pool: &Db) -> (String, String, Router, Uuid) {
    let (mine, ada) = (tenant(pool, "Clinic").await, tenant(pool, "Ada").await);
    let app = app(pool.clone(), mine);
    let (_, created) = call(app.clone(), "POST", format!("/api/v1/tenants/{mine}/invoices"), invoice(ada)).await;
    let id = created["id"].as_str().unwrap().to_string();
    let (_, added) = call(app.clone(), "POST", format!("/api/v1/tenants/{mine}/invoiceLines"), line(&id)).await;
    (id, added["id"].as_str().unwrap().to_string(), app, mine)
}

/// What `:open` does to the invoice, left uncommitted: it is locked and open, with a number.
async fn opening(pool: &Db, id: &str) -> Tx {
    let mut transaction = pool.begin().await.unwrap();
    Db::query("SELECT 1 FROM billing_invoices WHERE id = $1::uuid FOR UPDATE").sqlite("SELECT 1 FROM billing_invoices WHERE id = unhex(replace($1, '-', ''))").bind(id).execute(&mut transaction).await.unwrap();
    Db::query("UPDATE billing_invoices SET status = 'open', number = 'INV-1' WHERE id = $1::uuid").sqlite("UPDATE billing_invoices SET status = 'open', number = 'INV-1' WHERE id = unhex(replace($1, '-', ''))").bind(id).execute(&mut transaction).await.unwrap();
    transaction
}

/// Sends the request while the invoice is being opened, then lets the opening commit.
async fn while_opening(pool: &Db, id: &str, request: impl std::future::Future<Output = (StatusCode, Value)> + Send + 'static) -> StatusCode {
    let transaction = opening(pool, id).await;
    let sent = tokio::spawn(request);
    tokio::time::sleep(Duration::from_millis(300)).await;
    transaction.commit().await.unwrap();
    sent.await.unwrap().0
}

async fn stored(pool: &Db, id: &str) -> (String, Option<String>, bool) {
    Db::query_as("SELECT status, number, deleted_at IS NOT NULL FROM billing_invoices WHERE id = $1::uuid").sqlite("SELECT status, number, deleted_at IS NOT NULL FROM billing_invoices WHERE id = unhex(replace($1, '-', ''))").bind(id).fetch_one(pool).await.unwrap()
}

async fn lines(pool: &Db, id: &str) -> i64 {
    Db::query_scalar("SELECT count(*) FROM billing_invoice_lines WHERE invoice_id = $1::uuid AND deleted_at IS NULL").sqlite("SELECT count(*) FROM billing_invoice_lines WHERE invoice_id = unhex(replace($1, '-', '')) AND deleted_at IS NULL").bind(id).fetch_one(pool).await.unwrap()
}

dual!(a_line_added_while_the_invoice_opens_is_refused);
async fn a_line_added_while_the_invoice_opens_is_refused(pool: Db) {
    let (id, _, app, mine) = draft(&pool).await;

    let status = while_opening(&pool, &id, call(app, "POST", format!("/api/v1/tenants/{mine}/invoiceLines"), line(&id))).await;

    assert_eq!((status, lines(&pool, &id).await), (StatusCode::CONFLICT, 1));
}

dual!(a_line_changed_while_the_invoice_opens_is_refused);
async fn a_line_changed_while_the_invoice_opens_is_refused(pool: Db) {
    let (id, line_id, app, mine) = draft(&pool).await;

    let status = while_opening(&pool, &id, call(app, "PATCH", format!("/api/v1/tenants/{mine}/invoiceLines/{line_id}"), json!({"quantity": "9"}))).await;

    assert_eq!(status, StatusCode::CONFLICT);
}

dual!(an_invoice_replaced_while_it_opens_is_refused_and_stays_open);
async fn an_invoice_replaced_while_it_opens_is_refused_and_stays_open(pool: Db) {
    let (id, _, app, mine) = draft(&pool).await;
    let customer = tenant(&pool, "Bea").await;

    let status = while_opening(&pool, &id, call(app, "PUT", format!("/api/v1/tenants/{mine}/invoices/{id}"), invoice(customer))).await;

    assert_eq!((status, stored(&pool, &id).await), (StatusCode::CONFLICT, ("open".into(), Some("INV-1".into()), false)));
}

dual!(an_invoice_deleted_while_it_opens_is_refused_and_kept);
async fn an_invoice_deleted_while_it_opens_is_refused_and_kept(pool: Db) {
    let (id, _, app, mine) = draft(&pool).await;

    let status = while_opening(&pool, &id, call(app, "DELETE", format!("/api/v1/tenants/{mine}/invoices/{id}"), Value::Null)).await;

    assert_eq!((status, stored(&pool, &id).await), (StatusCode::CONFLICT, ("open".into(), Some("INV-1".into()), false)));
}
