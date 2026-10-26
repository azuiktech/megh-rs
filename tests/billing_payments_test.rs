//! B5e and S4 (docs/TPD-billing.md): payments over HTTP. A payment is money received for one open invoice; it gets its
//! number from the app's payment `Ids` when it is recorded, and a refund is kept on the payment.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::middleware::from_fn;
use axum::{Extension, Router};
use megh::auth::rest::AppContext;
use megh::billing::ids::{Formats, Middle, Numbering};
use megh::{authorizer, Grant};
use serde_json::{json, Value};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

async fn tenant(pool: &PgPool, name: &str) -> Uuid {
    sqlx::query_scalar("INSERT INTO tenants (name) VALUES ($1) RETURNING id").bind(name).fetch_one(pool).await.unwrap()
}

/// An invoice of 500.00 written straight to the table, as `:open` will leave it.
async fn invoice(pool: &PgPool, tenant: Uuid, status: &str) -> Uuid {
    let customer = tenant_of_customer(pool).await;
    sqlx::query_scalar("INSERT INTO billing_invoices (tenant_id, id, customer_id, status, date, due_date, currency, total, balance_due) VALUES ($1, $2, $3, $4, '2026-10-22', '2026-11-21', 'USD', 500000000, 500000000) RETURNING id")
        .bind(tenant)
        .bind(Uuid::now_v7())
        .bind(customer)
        .bind(status)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn tenant_of_customer(pool: &PgPool) -> Uuid {
    tenant(pool, "Ada").await
}

fn formats() -> Formats {
    Formats::new(Numbering::new("INV", "-", Middle::Year, 4).unwrap(), Numbering::new("PAY", "-", Middle::None, 4).unwrap())
}

fn app_with(pool: PgPool, grants: &[String], formats: Option<Formats>) -> Router {
    let (api, _) = megh::billing::router().split_for_parts();
    let router = Router::new().nest("/api/v1", api.layer(from_fn(authorizer))).layer(Extension(grants.iter().map(Grant::new).collect::<Vec<_>>()));
    let router = match formats {
        Some(formats) => router.layer(Extension(formats)),
        None => router,
    };
    router.with_state(AppContext { pool })
}

fn app(pool: PgPool, grants: &[String]) -> Router {
    app_with(pool, grants, Some(formats()))
}

fn grants(tenant: Uuid) -> Vec<String> {
    vec![format!("tenants:read:{tenant}"), "payments:*".into()]
}

async fn call(app: &Router, method: &str, uri: &str, body: Value) -> (StatusCode, Value) {
    let request = Request::builder().method(method).uri(uri);
    let request = match body {
        Value::Null => request.body(Body::empty()),
        body => request.header("content-type", "application/json").body(Body::from(body.to_string())),
    };
    let response = app.clone().oneshot(request.unwrap()).await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

fn payment(invoice: Uuid) -> Value {
    json!({"invoice_id": invoice, "date": "2026-10-22", "amount": 120000000, "method": "card", "status": "succeeded"})
}

fn with(mut body: Value, extra: Value) -> Value {
    body.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
    body
}

#[sqlx::test(migrations = false)]
async fn a_payment_is_for_an_open_invoice_of_the_tenant(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let (mine, other) = (tenant(&pool, "Clinic").await, tenant(&pool, "Other").await);
    let app = app(pool.clone(), &[grants(mine), grants(other)].concat());
    let payments = format!("/api/v1/tenants/{mine}/payments");
    let owed = invoice(&pool, mine, "open").await;

    let (status, created) = call(&app, "POST", &payments, with(payment(owed), json!({"tenant_id": other}))).await;
    assert_eq!(status, StatusCode::OK, "{created}");
    assert_eq!((created["tenant_id"].clone(), created["refunded"].clone(), created["number"].clone(), created["invoice_id"].clone()), (json!(mine), json!(0), json!("PAY-0001"), json!(owed)));

    for status in ["draft", "void"] {
        assert_eq!(call(&app, "POST", &payments, payment(invoice(&pool, mine, status).await)).await.0, StatusCode::CONFLICT, "an invoice that is {status}");
    }
    let wrong = [
        ("another tenant's invoice", payment(invoice(&pool, other, "open").await)),
        ("no invoice", payment(Uuid::now_v7())),
        ("a zero amount", with(payment(owed), json!({"amount": 0}))),
        ("a negative amount", with(payment(owed), json!({"amount": -5}))),
        ("more refunded than paid", with(payment(owed), json!({"refunded": 120000001}))),
        ("a negative refund", with(payment(owed), json!({"refunded": -1}))),
        ("a number of its own", with(payment(owed), json!({"number": "PAY-9"}))),
        ("an unknown method", with(payment(owed), json!({"method": "bitcoin"}))),
        ("an unknown status", with(payment(owed), json!({"status": "paid"}))),
        ("an empty body", json!({})),
    ];
    for (what, body) in wrong {
        assert_eq!(call(&app, "POST", &payments, body).await.0, StatusCode::BAD_REQUEST, "{what}");
    }
    let (_, second) = call(&app, "POST", &payments, payment(owed)).await;
    assert_eq!(second["number"], "PAY-0002", "the refused ones used no number");
}

#[sqlx::test(migrations = false)]
async fn a_payment_is_read_replaced_patched_and_deleted(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let (mine, other) = (tenant(&pool, "Clinic").await, tenant(&pool, "Other").await);
    let (mine_app, other_app) = (app(pool.clone(), &grants(mine)), app(pool.clone(), &grants(other)));
    let (owed, second) = (invoice(&pool, mine, "open").await, invoice(&pool, mine, "open").await);
    let (_, created) = call(&mine_app, "POST", &format!("/api/v1/tenants/{mine}/payments"), payment(owed)).await;
    let id = created["id"].as_str().unwrap().to_string();
    let item = format!("/api/v1/tenants/{mine}/payments/{id}");

    assert_eq!(call(&mine_app, "GET", &item, Value::Null).await.1["id"], json!(id));
    let (status, put) = call(&mine_app, "PUT", &item, with(payment(owed), json!({"method": "cash"}))).await;
    assert_eq!((status, put["method"].clone(), put["number"].clone()), (StatusCode::OK, json!("cash"), json!("PAY-0001")), "a PUT keeps the number: {put}");
    let (status, patched) = call(&mine_app, "PATCH", &item, json!({"refunded": 20000000, "status": "succeeded"})).await;
    assert_eq!((status, patched["refunded"].clone(), patched["method"].clone(), patched["number"].clone()), (StatusCode::OK, json!(20000000), json!("cash"), json!("PAY-0001")), "{patched}");

    assert_eq!(call(&mine_app, "PUT", &item, with(payment(owed), json!({"number": "PAY-9"}))).await.0, StatusCode::BAD_REQUEST, "the number is not changed by a PUT");
    assert_eq!(call(&mine_app, "PATCH", &item, json!({"number": "PAY-9"})).await.0, StatusCode::BAD_REQUEST, "nor by a PATCH");
    assert_eq!(call(&mine_app, "PATCH", &item, json!({"invoice_id": second})).await.0, StatusCode::BAD_REQUEST, "a payment stays with its invoice");
    assert_eq!(call(&mine_app, "PATCH", &item, json!({"amount": 0})).await.0, StatusCode::BAD_REQUEST, "a patch is checked too");
    assert_eq!(call(&mine_app, "PATCH", &item, json!({"refunded": 999999999999i64})).await.0, StatusCode::BAD_REQUEST);
    assert_eq!(call(&mine_app, "GET", &format!("/api/v1/tenants/{mine}/payments/{}", Uuid::now_v7()), Value::Null).await.0, StatusCode::NOT_FOUND);
    assert_eq!(call(&other_app, "GET", &format!("/api/v1/tenants/{other}/payments/{id}"), Value::Null).await.0, StatusCode::NOT_FOUND, "another tenant's payment");
    assert_eq!(call(&other_app, "DELETE", &format!("/api/v1/tenants/{other}/payments/{id}"), Value::Null).await.0, StatusCode::NOT_FOUND);

    let listed = call(&mine_app, "GET", &format!("/api/v1/tenants/{mine}/payments?filter=invoice_id%20%3D%20%22{owed}%22"), Value::Null).await.1;
    assert_eq!(listed["items"].as_array().unwrap().len(), 1);
    assert_eq!(call(&mine_app, "DELETE", &item, Value::Null).await.0, StatusCode::OK);
    assert_eq!(call(&mine_app, "GET", &item, Value::Null).await.0, StatusCode::NOT_FOUND);
    assert_eq!(call(&mine_app, "PUT", &item, payment(owed)).await.0, StatusCode::NOT_FOUND, "a deleted payment stays deleted");
}

#[sqlx::test(migrations = false)]
async fn authorizer_guards_payments(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let (mine, other) = (tenant(&pool, "Clinic").await, tenant(&pool, "Other").await);
    let collection = format!("/api/v1/tenants/{mine}/payments");
    let read_only = app(pool.clone(), &[format!("tenants:read:{mine}"), "payments:read".into()]);
    assert_eq!(call(&read_only, "GET", &collection, Value::Null).await.0, StatusCode::OK);
    assert_eq!(call(&read_only, "POST", &collection, payment(Uuid::now_v7())).await.0, StatusCode::FORBIDDEN);
    let mine_only = app(pool.clone(), &grants(mine));
    assert_eq!(call(&mine_only, "GET", &format!("/api/v1/tenants/{other}/payments"), Value::Null).await.0, StatusCode::FORBIDDEN);
}

#[test]
fn payments_are_documented_with_their_grants() {
    let (_, api) = megh::billing::router().split_for_parts();
    for path in ["/tenants/{tenant_id}/payments", "/tenants/{tenant_id}/payments/{id}"] {
        let item = api.paths.paths.get(path).unwrap_or_else(|| panic!("{path}"));
        for operation in [&item.get, &item.post, &item.put, &item.patch, &item.delete].into_iter().flatten() {
            assert!(operation.security.as_ref().is_some_and(|security| !security.is_empty()), "{path}");
        }
    }
    assert!(api.tags.unwrap_or_default().into_iter().any(|tag| tag.name == "payments"));
}
