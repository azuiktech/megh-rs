//! S4 (docs/TPD-billing.md): the balance of an invoice follows its payments. `amount_paid` is the sum of `amount` less
//! `refunded` over its processing and succeeded payments, `balance_due` is the total less that, and a payment never
//! pays more than is owed. Every change to a payment is one transaction with the refresh of its invoice.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::middleware::from_fn;
use axum::{Extension, Router};
use megh::auth::rest::AppContext;
use megh::billing::ids::{Formats, Middle, Numbering};
use megh::{authorizer, Grant};
use serde_json::{json, Value};
use sqlx::PgPool;
use tower::util::MapRequestLayer;
use tower::{Layer, ServiceExt};
use uuid::Uuid;

const HUNDRED: i64 = 100_000_000;

async fn tenant(pool: &PgPool, name: &str) -> Uuid {
    sqlx::query_scalar("INSERT INTO tenants (name) VALUES ($1) RETURNING id").bind(name).fetch_one(pool).await.unwrap()
}

/// An open invoice of 100.00.
async fn invoice(pool: &PgPool, tenant: Uuid, customer: Uuid) -> Uuid {
    sqlx::query_scalar("INSERT INTO billing_invoices (tenant_id, id, customer_id, number, status, date, due_date, currency, total, balance_due) VALUES ($1, $2, $3, $4, 'open', '2026-10-22', '2026-11-21', 'USD', $5, $5) RETURNING id")
        .bind(tenant)
        .bind(Uuid::now_v7())
        .bind(customer)
        .bind(Uuid::now_v7().to_string())
        .bind(HUNDRED)
        .fetch_one(pool)
        .await
        .unwrap()
}

fn app(pool: PgPool, tenant: Uuid) -> Router {
    let formats = Formats::new(Numbering::new("INV", "-", Middle::Year, 4).unwrap(), Numbering::new("PAY", "-", Middle::None, 4).unwrap());
    let (api, _) = megh::billing::router().split_for_parts();
    let grants = vec![format!("tenants:read:{tenant}"), "payments:*".to_string(), "invoices:*".to_string()];
    Router::new()
        .nest("/api/v1", api.layer(from_fn(authorizer)))
        .layer(Extension(grants.iter().map(Grant::new).collect::<Vec<_>>()))
        .layer(Extension(formats))
        .with_state(AppContext { pool })
}

async fn call(app: &Router, method: &str, uri: &str, body: Value) -> (StatusCode, Value) {
    let request = Request::builder().method(method).uri(uri);
    let request = match body {
        Value::Null => request.body(Body::empty()),
        body => request.header("content-type", "application/json").body(Body::from(body.to_string())),
    };
    let service = MapRequestLayer::new(megh::booking::rewrite_aip_136).layer(app.clone());
    let response = service.oneshot(request.unwrap()).await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

fn pay(invoice: Uuid, units: i64, status: &str) -> Value {
    json!({"invoice_id": invoice, "date": "2026-10-22", "amount": units * 1_000_000, "method": "card", "status": status})
}

async fn amounts(app: &Router, tenant: Uuid, invoice: Uuid) -> (i64, i64) {
    let stored = call(app, "GET", &format!("/api/v1/tenants/{tenant}/invoices/{invoice}"), Value::Null).await.1;
    (stored["amount_paid"].as_i64().unwrap() / 1_000_000, stored["balance_due"].as_i64().unwrap() / 1_000_000)
}

async fn record(app: &Router, tenant: Uuid, body: Value) -> (StatusCode, Value) {
    call(app, "POST", &format!("/api/v1/tenants/{tenant}/payments"), body).await
}

fn item(tenant: Uuid, payment: &Value) -> String {
    format!("/api/v1/tenants/{tenant}/payments/{}", payment["id"].as_str().unwrap())
}

#[sqlx::test(migrations = false)]
async fn paying_lowers_what_is_owed_until_nothing_is(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let (mine, customer) = (tenant(&pool, "Clinic").await, tenant(&pool, "Ada").await);
    let (app, owed) = (app(pool.clone(), mine), invoice(&pool, mine, customer).await);
    assert_eq!(amounts(&app, mine, owed).await, (0, 100));

    assert_eq!(record(&app, mine, pay(owed, 30, "succeeded")).await.0, StatusCode::OK);
    assert_eq!(amounts(&app, mine, owed).await, (30, 70));
    let (status, last) = record(&app, mine, pay(owed, 70, "succeeded")).await;
    assert_eq!((status, last["number"].clone()), (StatusCode::OK, json!("PAY-0002")));
    assert_eq!(amounts(&app, mine, owed).await, (100, 0));

    assert_eq!(record(&app, mine, pay(owed, 1, "succeeded")).await.0, StatusCode::CONFLICT, "nothing is owed");
    assert_eq!(amounts(&app, mine, owed).await, (100, 0), "a refused payment changes nothing");
    let listed = call(&app, "GET", &format!("/api/v1/tenants/{mine}/payments?filter=invoice_id%20%3D%20%22{owed}%22"), Value::Null).await.1;
    assert_eq!(listed["items"].as_array().unwrap().len(), 2, "and is not recorded");
}

#[sqlx::test(migrations = false)]
async fn a_payment_that_is_more_than_is_owed_is_refused_and_uses_no_number(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let (mine, customer) = (tenant(&pool, "Clinic").await, tenant(&pool, "Ada").await);
    let (app, owed) = (app(pool.clone(), mine), invoice(&pool, mine, customer).await);
    assert_eq!(record(&app, mine, pay(owed, 101, "succeeded")).await.0, StatusCode::CONFLICT);
    let (status, first) = record(&app, mine, pay(owed, 100, "succeeded")).await;
    assert_eq!((status, first["number"].clone()), (StatusCode::OK, json!("PAY-0001")), "the refused payment took no number");
}

#[sqlx::test(migrations = false)]
async fn only_processing_and_succeeded_payments_count(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let (mine, customer) = (tenant(&pool, "Clinic").await, tenant(&pool, "Ada").await);
    let (app, owed) = (app(pool.clone(), mine), invoice(&pool, mine, customer).await);

    let (_, processing) = record(&app, mine, pay(owed, 20, "processing")).await;
    assert_eq!(amounts(&app, mine, owed).await, (20, 80), "money on its way counts");
    record(&app, mine, pay(owed, 30, "failed")).await;
    record(&app, mine, pay(owed, 40, "void")).await;
    assert_eq!(amounts(&app, mine, owed).await, (20, 80), "a failed or void payment does not");

    call(&app, "PATCH", &item(mine, &processing), json!({"status": "failed"})).await;
    assert_eq!(amounts(&app, mine, owed).await, (0, 100), "a payment that fails stops counting");
    call(&app, "PATCH", &item(mine, &processing), json!({"status": "succeeded"})).await;
    assert_eq!(amounts(&app, mine, owed).await, (20, 80));
}

#[sqlx::test(migrations = false)]
async fn a_refund_gives_back_what_was_paid(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let (mine, customer) = (tenant(&pool, "Clinic").await, tenant(&pool, "Ada").await);
    let (app, owed) = (app(pool.clone(), mine), invoice(&pool, mine, customer).await);
    let (_, paid) = record(&app, mine, pay(owed, 50, "succeeded")).await;
    assert_eq!(amounts(&app, mine, owed).await, (50, 50));

    assert_eq!(call(&app, "PATCH", &item(mine, &paid), json!({"refunded": 20000000})).await.0, StatusCode::OK);
    assert_eq!(amounts(&app, mine, owed).await, (30, 70), "20 given back");
    assert_eq!(call(&app, "PATCH", &item(mine, &paid), json!({"refunded": 50000000})).await.0, StatusCode::OK);
    assert_eq!(amounts(&app, mine, owed).await, (0, 100), "all given back");
    assert_eq!(call(&app, "PATCH", &item(mine, &paid), json!({"refunded": 50000001})).await.0, StatusCode::BAD_REQUEST, "no more than was paid");
    assert_eq!(amounts(&app, mine, owed).await, (0, 100));
    assert_eq!(call(&app, "POST", &format!("/api/v1/tenants/{mine}/invoices/{owed}:void"), json!({})).await.0, StatusCode::OK, "nothing is left paid, so it can be voided");
}

#[sqlx::test(migrations = false)]
async fn changing_a_payment_refreshes_the_invoice_and_cannot_overpay(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let (mine, customer) = (tenant(&pool, "Clinic").await, tenant(&pool, "Ada").await);
    let (app, owed) = (app(pool.clone(), mine), invoice(&pool, mine, customer).await);
    let (_, paid) = record(&app, mine, pay(owed, 50, "succeeded")).await;
    let here = item(mine, &paid);

    let (status, put) = call(&app, "PUT", &here, pay(owed, 80, "succeeded")).await;
    assert_eq!((status, put["number"].clone()), (StatusCode::OK, json!("PAY-0001")), "{put}");
    assert_eq!(amounts(&app, mine, owed).await, (80, 20));
    assert_eq!(call(&app, "PUT", &here, pay(owed, 120, "succeeded")).await.0, StatusCode::CONFLICT, "more than is owed");
    assert_eq!(call(&app, "PATCH", &here, json!({"amount": 101000000})).await.0, StatusCode::CONFLICT, "a patch too");
    assert_eq!(amounts(&app, mine, owed).await, (80, 20), "a refused change changes nothing");
    assert_eq!(call(&app, "GET", &here, Value::Null).await.1["amount"], json!(80000000));
}

#[sqlx::test(migrations = false)]
async fn deleting_a_payment_gives_the_invoice_its_balance_back(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let (mine, customer) = (tenant(&pool, "Clinic").await, tenant(&pool, "Ada").await);
    let (app, owed) = (app(pool.clone(), mine), invoice(&pool, mine, customer).await);
    let (_, first) = record(&app, mine, pay(owed, 30, "succeeded")).await;
    record(&app, mine, pay(owed, 20, "succeeded")).await;
    assert_eq!(amounts(&app, mine, owed).await, (50, 50));
    assert_eq!(call(&app, "DELETE", &item(mine, &first), Value::Null).await.0, StatusCode::OK);
    assert_eq!(amounts(&app, mine, owed).await, (20, 80), "a deleted payment no longer counts");
}

#[sqlx::test(migrations = false)]
async fn payments_made_at_the_same_time_never_pay_more_than_is_owed(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let (mine, customer) = (tenant(&pool, "Clinic").await, tenant(&pool, "Ada").await);
    let (app, owed) = (app(pool.clone(), mine), invoice(&pool, mine, customer).await);
    let tasks: Vec<_> = (0..6)
        .map(|_| {
            let app = app.clone();
            tokio::spawn(async move { record(&app, mine, pay(owed, 30, "succeeded")).await })
        })
        .collect();
    let mut statuses = vec![];
    let mut numbers = vec![];
    for task in tasks {
        let (status, row) = task.await.unwrap();
        statuses.push(status);
        if status == StatusCode::OK {
            numbers.push(row["number"].as_str().unwrap().to_string());
        }
    }
    assert_eq!(statuses.iter().filter(|status| **status == StatusCode::OK).count(), 3, "three of 30 fit in 100");
    assert!(statuses.iter().all(|status| *status == StatusCode::OK || *status == StatusCode::CONFLICT));
    numbers.sort();
    assert_eq!(numbers, ["PAY-0001", "PAY-0002", "PAY-0003"], "no number skipped or repeated");
    assert_eq!(amounts(&app, mine, owed).await, (90, 10));
}

#[sqlx::test(migrations = false)]
async fn each_tenants_payments_are_numbered_apart_and_the_app_must_give_its_ids(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let (mine, other, customer) = (tenant(&pool, "Clinic").await, tenant(&pool, "Other").await, tenant(&pool, "Ada").await);
    let (mine_app, other_app) = (app(pool.clone(), mine), app(pool.clone(), other));
    let (mine_invoice, other_invoice) = (invoice(&pool, mine, customer).await, invoice(&pool, other, customer).await);
    assert_eq!(record(&mine_app, mine, pay(mine_invoice, 10, "succeeded")).await.1["number"], "PAY-0001");
    assert_eq!(record(&other_app, other, pay(other_invoice, 10, "succeeded")).await.1["number"], "PAY-0001", "another tenant has its own numbers");

    let (api, _) = megh::billing::router().split_for_parts();
    let grants = vec![format!("tenants:read:{mine}"), "payments:*".to_string(), "invoices:*".to_string()];
    let without = Router::new().nest("/api/v1", api.layer(from_fn(authorizer))).layer(Extension(grants.iter().map(Grant::new).collect::<Vec<_>>())).with_state(AppContext { pool: pool.clone() });
    assert_eq!(record(&without, mine, pay(mine_invoice, 10, "succeeded")).await.0, StatusCode::INTERNAL_SERVER_ERROR, "no Formats extension");
    assert_eq!(amounts(&without, mine, mine_invoice).await, (10, 90), "nothing changed");
}
