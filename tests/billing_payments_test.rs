//! B5e (docs/TPD-billing.md): payments over HTTP. A payment is money received for one open invoice; a refund is kept
//! on the payment.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::middleware::from_fn;
use axum::{Extension, Router};
use megh::auth::rest::AppContext;
use megh::{authorizer, Grant};
use serde_json::{json, Value};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

async fn org(pool: &PgPool, name: &str) -> Uuid {
    sqlx::query_scalar("INSERT INTO organizations (name) VALUES ($1) RETURNING id").bind(name).fetch_one(pool).await.unwrap()
}

/// An invoice written straight to the table, as `:open` will leave it.
async fn invoice(pool: &PgPool, org: Uuid, status: &str) -> Uuid {
    let customer = org_of_customer(pool).await;
    sqlx::query_scalar("INSERT INTO billing_invoices (org_id, id, customer_id, status, date, due_date, currency) VALUES ($1, $2, $3, $4, '2026-10-22', '2026-11-21', 'USD') RETURNING id")
        .bind(org)
        .bind(Uuid::now_v7())
        .bind(customer)
        .bind(status)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn org_of_customer(pool: &PgPool) -> Uuid {
    org(pool, "Ada").await
}

fn app(pool: PgPool, grants: &[String]) -> Router {
    let (api, _) = megh::billing::router().split_for_parts();
    Router::new()
        .nest("/api/v1", api.layer(from_fn(authorizer)))
        .layer(Extension(grants.iter().map(Grant::new).collect::<Vec<_>>()))
        .with_state(AppContext { pool })
}

fn grants(org: Uuid) -> Vec<String> {
    vec![format!("orgs:read:{org}"), "payments:*".into()]
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
async fn a_payment_is_for_an_open_invoice_of_the_org(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let (mine, other) = (org(&pool, "Clinic").await, org(&pool, "Other").await);
    let app = app(pool.clone(), &[grants(mine), grants(other)].concat());
    let payments = format!("/api/v1/orgs/{mine}/payments");
    let owed = invoice(&pool, mine, "open").await;

    let (status, created) = call(&app, "POST", &payments, with(payment(owed), json!({"org_id": other}))).await;
    assert_eq!(status, StatusCode::OK, "{created}");
    assert_eq!((created["org_id"].clone(), created["refunded"].clone(), created["number"].clone(), created["invoice_id"].clone()), (json!(mine), json!(0), Value::Null, json!(owed)));

    for status in ["draft", "void"] {
        assert_eq!(call(&app, "POST", &payments, payment(invoice(&pool, mine, status).await)).await.0, StatusCode::CONFLICT, "an invoice that is {status}");
    }
    let wrong = [
        ("another org's invoice", payment(invoice(&pool, other, "open").await)),
        ("no invoice", payment(Uuid::now_v7())),
        ("a zero amount", with(payment(owed), json!({"amount": 0}))),
        ("a negative amount", with(payment(owed), json!({"amount": -5}))),
        ("more refunded than paid", with(payment(owed), json!({"refunded": 120000001}))),
        ("a negative refund", with(payment(owed), json!({"refunded": -1}))),
        ("a number", with(payment(owed), json!({"number": 5}))),
        ("an unknown method", with(payment(owed), json!({"method": "bitcoin"}))),
        ("an unknown status", with(payment(owed), json!({"status": "paid"}))),
        ("an empty body", json!({})),
    ];
    for (what, body) in wrong {
        assert_eq!(call(&app, "POST", &payments, body).await.0, StatusCode::BAD_REQUEST, "{what}");
    }
}

#[sqlx::test(migrations = false)]
async fn a_payment_is_read_replaced_patched_and_deleted(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let (mine, other) = (org(&pool, "Clinic").await, org(&pool, "Other").await);
    let (mine_app, other_app) = (app(pool.clone(), &grants(mine)), app(pool.clone(), &grants(other)));
    let (owed, second) = (invoice(&pool, mine, "open").await, invoice(&pool, mine, "open").await);
    let (_, created) = call(&mine_app, "POST", &format!("/api/v1/orgs/{mine}/payments"), payment(owed)).await;
    let id = created["id"].as_str().unwrap().to_string();
    let item = format!("/api/v1/orgs/{mine}/payments/{id}");

    assert_eq!(call(&mine_app, "GET", &item, Value::Null).await.1["id"], json!(id));
    let (status, put) = call(&mine_app, "PUT", &item, with(payment(owed), json!({"method": "cash"}))).await;
    assert_eq!((status, put["method"].clone()), (StatusCode::OK, json!("cash")), "{put}");
    let (status, patched) = call(&mine_app, "PATCH", &item, json!({"refunded": 20000000, "status": "succeeded"})).await;
    assert_eq!((status, patched["refunded"].clone(), patched["method"].clone()), (StatusCode::OK, json!(20000000), json!("cash")), "{patched}");

    assert_eq!(call(&mine_app, "PATCH", &item, json!({"invoice_id": second})).await.0, StatusCode::BAD_REQUEST, "a payment stays with its invoice");
    assert_eq!(call(&mine_app, "PATCH", &item, json!({"amount": 0})).await.0, StatusCode::BAD_REQUEST, "a patch is checked too");
    assert_eq!(call(&mine_app, "PATCH", &item, json!({"refunded": 999999999999i64})).await.0, StatusCode::BAD_REQUEST);
    assert_eq!(call(&mine_app, "GET", &format!("/api/v1/orgs/{mine}/payments/{}", Uuid::now_v7()), Value::Null).await.0, StatusCode::NOT_FOUND);
    assert_eq!(call(&other_app, "GET", &format!("/api/v1/orgs/{other}/payments/{id}"), Value::Null).await.0, StatusCode::NOT_FOUND, "another org's payment");

    let listed = call(&mine_app, "GET", &format!("/api/v1/orgs/{mine}/payments?filter=invoice_id%20%3D%20%22{owed}%22"), Value::Null).await.1;
    assert_eq!(listed["items"].as_array().unwrap().len(), 1);
    assert_eq!(call(&mine_app, "DELETE", &item, Value::Null).await.0, StatusCode::OK);
    assert_eq!(call(&mine_app, "GET", &item, Value::Null).await.0, StatusCode::NOT_FOUND);
    assert_eq!(call(&mine_app, "PUT", &item, payment(owed)).await.0, StatusCode::NOT_FOUND, "a deleted payment stays deleted");
}

#[sqlx::test(migrations = false)]
async fn authorizer_guards_payments(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let (mine, other) = (org(&pool, "Clinic").await, org(&pool, "Other").await);
    let collection = format!("/api/v1/orgs/{mine}/payments");
    let read_only = app(pool.clone(), &[format!("orgs:read:{mine}"), "payments:read".into()]);
    assert_eq!(call(&read_only, "GET", &collection, Value::Null).await.0, StatusCode::OK);
    assert_eq!(call(&read_only, "POST", &collection, payment(Uuid::now_v7())).await.0, StatusCode::FORBIDDEN);
    let mine_only = app(pool.clone(), &grants(mine));
    assert_eq!(call(&mine_only, "GET", &format!("/api/v1/orgs/{other}/payments"), Value::Null).await.0, StatusCode::FORBIDDEN);
}

#[test]
fn payments_are_documented_with_their_grants() {
    let (_, api) = megh::billing::router().split_for_parts();
    for path in ["/orgs/{org_id}/payments", "/orgs/{org_id}/payments/{id}"] {
        let item = api.paths.paths.get(path).unwrap_or_else(|| panic!("{path}"));
        for operation in [&item.get, &item.post, &item.put, &item.patch, &item.delete].into_iter().flatten() {
            assert!(operation.security.as_ref().is_some_and(|security| !security.is_empty()), "{path}");
        }
    }
    assert!(api.tags.unwrap_or_default().into_iter().any(|tag| tag.name == "payments"));
}
