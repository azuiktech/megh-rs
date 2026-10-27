//! S6d (docs/TPD-billing.md section 7, #227): stock follows payments. When an invoice gets a succeeded payment, its
//! product lines leave the tenant's stock (receivable) or enter it (payable); a full refund, a failure or a void undoes
//! that. Selling more than is held is a 409.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::middleware::from_fn;
use axum::{Extension, Router};
use megh::auth::rest::AppContext;
use megh::billing::ids::{Formats, Middle, Numbering};
use megh::{Grant, authorizer};
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::util::MapRequestLayer;
use tower::{Layer, ServiceExt};
use uuid::Uuid;

const HUNDRED: i64 = 100_000_000;

async fn tenant(pool: &PgPool, name: &str) -> Uuid {
    sqlx::query_scalar("INSERT INTO tenants (name) VALUES ($1) RETURNING id")
        .bind(name)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn product(pool: &PgPool, tenant: Uuid, sku: &str) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO products (tenant_id, id, sku, name, unit, price, currency) VALUES ($1, uuidv7(), $2, $2, 'tube', 12000000, 'USD') RETURNING id",
    )
    .bind(tenant)
    .bind(sku)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn hold(pool: &PgPool, tenant: Uuid, product: Uuid, on_hand: i32) {
    sqlx::query(
        "INSERT INTO stocks (tenant_id, id, product_id, on_hand) VALUES ($1, uuidv7(), $2, $3)",
    )
    .bind(tenant)
    .bind(product)
    .bind(on_hand)
    .execute(pool)
    .await
    .unwrap();
}

async fn on_hand(pool: &PgPool, tenant: Uuid, product: Uuid) -> Option<String> {
    sqlx::query_scalar("SELECT on_hand::text FROM stocks WHERE tenant_id = $1 AND product_id = $2")
        .bind(tenant)
        .bind(product)
        .fetch_optional(pool)
        .await
        .unwrap()
}

/// An open invoice of 100.00 with one line of `quantity` of the product.
async fn invoice(pool: &PgPool, tenant: Uuid, kind: &str, product: Uuid, quantity: i32) -> Uuid {
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO billing_invoices (tenant_id, id, party_id, party_kind, kind, number, status, date, due_date, currency, total, balance_due)
         VALUES ($1, uuidv7(), $1, 'tenant', $2, uuidv7()::text, 'open', '2026-10-27', '2026-11-26', 'USD', $3, $3) RETURNING id",
    )
    .bind(tenant)
    .bind(kind)
    .bind(HUNDRED)
    .fetch_one(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO billing_invoice_lines (tenant_id, id, invoice_id, position, product_id, description, quantity, unit_price)
         VALUES ($1, uuidv7(), $2, 1, $3, 'Hair colour', $4, 12000000)",
    )
    .bind(tenant)
    .bind(id)
    .bind(product)
    .bind(quantity)
    .execute(pool)
    .await
    .unwrap();
    id
}

fn app(pool: PgPool, tenant: Uuid) -> Router {
    let formats = Formats::new(
        Numbering::new("INV", "-", Middle::Year, 4).unwrap(),
        Numbering::new("PAY", "-", Middle::None, 4).unwrap(),
    );
    let (api, _) = megh::billing::router().split_for_parts();
    let grants = vec![
        Grant::new(format!("tenants:read:{tenant}")),
        Grant::new("payments:*"),
        Grant::new("invoices:*"),
    ];
    Router::new()
        .nest("/api/v1", api.layer(from_fn(authorizer)))
        .layer(Extension(grants))
        .layer(Extension(formats))
        .with_state(AppContext { pool })
}

async fn call(app: &Router, method: &str, uri: &str, body: Value) -> (StatusCode, Value) {
    let request = Request::builder().method(method).uri(uri);
    let request = match body {
        Value::Null => request.body(Body::empty()),
        body => request
            .header("content-type", "application/json")
            .body(Body::from(body.to_string())),
    };
    let service = MapRequestLayer::new(megh::booking::rewrite_aip_136).layer(app.clone());
    let response = service.oneshot(request.unwrap()).await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

fn pay(invoice: Uuid, status: &str) -> Value {
    json!({"invoice_id": invoice, "date": "2026-10-27", "amount": HUNDRED, "method": "card", "status": status})
}

#[sqlx::test(migrations = false)]
async fn a_paid_sale_takes_its_products_from_stock_until_undone(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let mine = tenant(&pool, "Salon").await;
    let colour = product(&pool, mine, "WEL-60").await;
    hold(&pool, mine, colour, 4).await;
    let app = app(pool.clone(), mine);
    let payments = format!("/api/v1/tenants/{mine}/payments");
    let sale = invoice(&pool, mine, "receivable", colour, 2).await;

    let (status, waiting) = call(&app, "POST", &payments, pay(sale, "processing")).await;
    assert_eq!(status, StatusCode::OK, "{waiting}");
    assert_eq!(
        on_hand(&pool, mine, colour).await.as_deref(),
        Some("4"),
        "not until it succeeds"
    );

    let item = format!("{payments}/{}", waiting["id"].as_str().unwrap());
    let succeed = |app: Router, item: String| async move {
        call(&app, "PATCH", &item, json!({"status": "succeeded"})).await
    };
    assert_eq!(succeed(app.clone(), item.clone()).await.0, StatusCode::OK);
    assert_eq!(on_hand(&pool, mine, colour).await.as_deref(), Some("2"));

    assert_eq!(
        call(&app, "PATCH", &item, json!({"refunded": HUNDRED / 2}))
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(
        on_hand(&pool, mine, colour).await.as_deref(),
        Some("2"),
        "a part refund keeps the goods sold"
    );
    assert_eq!(
        call(&app, "PATCH", &item, json!({"refunded": HUNDRED}))
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(
        on_hand(&pool, mine, colour).await.as_deref(),
        Some("4"),
        "a full refund gives them back"
    );

    assert_eq!(
        call(&app, "PATCH", &item, json!({"refunded": 0})).await.0,
        StatusCode::OK
    );
    assert_eq!(on_hand(&pool, mine, colour).await.as_deref(), Some("2"));
    assert_eq!(
        call(&app, "PATCH", &item, json!({"status": "failed"}))
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(
        on_hand(&pool, mine, colour).await.as_deref(),
        Some("4"),
        "a failure gives them back"
    );
    assert_eq!(succeed(app.clone(), item.clone()).await.0, StatusCode::OK);
    assert_eq!(
        call(&app, "PATCH", &item, json!({"status": "void"}))
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(
        on_hand(&pool, mine, colour).await.as_deref(),
        Some("4"),
        "a void gives them back"
    );
    assert_eq!(succeed(app.clone(), item.clone()).await.0, StatusCode::OK);
    assert_eq!(
        call(&app, "DELETE", &item, Value::Null).await.0,
        StatusCode::OK
    );
    assert_eq!(
        on_hand(&pool, mine, colour).await.as_deref(),
        Some("4"),
        "a deleted payment gives them back"
    );
}

#[sqlx::test(migrations = false)]
async fn selling_more_than_is_held_is_refused(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let mine = tenant(&pool, "Salon").await;
    let colour = product(&pool, mine, "WEL-60").await;
    let gloves = product(&pool, mine, "GLV").await;
    hold(&pool, mine, colour, 1).await;
    let app = app(pool.clone(), mine);
    let payments = format!("/api/v1/tenants/{mine}/payments");

    let too_many = invoice(&pool, mine, "receivable", colour, 2).await;
    let (status, refused) = call(&app, "POST", &payments, pay(too_many, "succeeded")).await;
    assert_eq!(status, StatusCode::CONFLICT, "{refused}");
    assert_eq!(on_hand(&pool, mine, colour).await.as_deref(), Some("1"));
    let (_, page) = call(&app, "GET", &payments, Value::Null).await;
    assert!(
        page["items"].as_array().unwrap().is_empty(),
        "nothing recorded"
    );

    let none_held = invoice(&pool, mine, "receivable", gloves, 1).await;
    assert_eq!(
        call(&app, "POST", &payments, pay(none_held, "succeeded"))
            .await
            .0,
        StatusCode::CONFLICT,
        "nothing of it is held"
    );
    assert_eq!(on_hand(&pool, mine, gloves).await, None);
}

#[sqlx::test(migrations = false)]
async fn a_paid_bill_adds_its_products_to_stock(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let mine = tenant(&pool, "Salon").await;
    let brand = tenant(&pool, "Brand").await;
    let colour = product(&pool, brand, "WEL-60").await;
    let app = app(pool.clone(), mine);
    let payments = format!("/api/v1/tenants/{mine}/payments");

    let bill = invoice(&pool, mine, "payable", colour, 3).await;
    let (status, paid) = call(&app, "POST", &payments, pay(bill, "succeeded")).await;
    assert_eq!(status, StatusCode::OK, "{paid}");
    assert_eq!(
        on_hand(&pool, mine, colour).await.as_deref(),
        Some("3"),
        "the first stock of another tenant's product"
    );

    let again = invoice(&pool, mine, "payable", colour, 2).await;
    let (_, second) = call(&app, "POST", &payments, pay(again, "succeeded")).await;
    assert_eq!(on_hand(&pool, mine, colour).await.as_deref(), Some("5"));
    let item = format!("{payments}/{}", second["id"].as_str().unwrap());
    assert_eq!(
        call(&app, "PATCH", &item, json!({"status": "failed"}))
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(
        on_hand(&pool, mine, colour).await.as_deref(),
        Some("3"),
        "undone the other way"
    );
}
