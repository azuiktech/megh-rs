//! S6e (docs/TPD-billing.md section 7, #227): an appointment lists the products it uses. Booking it, or changing its
//! products, needs that much on hand at the tenant (409 otherwise); a booked appointment holds no stock.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::middleware::from_fn;
use axum::{Extension, Router};
use megh::auth::rest::AppContext;
use megh::{Grant, authorizer};
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

async fn one(pool: &PgPool, sql: &str, tenant: Uuid) -> Uuid {
    sqlx::query_scalar(sql)
        .bind(tenant)
        .fetch_one(pool)
        .await
        .unwrap()
}

fn app(pool: PgPool, tenant: Uuid) -> Router {
    let (api, _) = megh::booking::router().split_for_parts();
    let grants = vec![
        Grant::new(format!("tenants:read:{tenant}")),
        Grant::new("appointments:*"),
    ];
    Router::new()
        .nest("/api/v1", api.layer(from_fn(authorizer)))
        .layer(Extension(grants))
        .with_state(AppContext { pool })
}

async fn call(app: &Router, method: &str, uri: &str, body: Value) -> (StatusCode, Value) {
    let request = Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()));
    let response = app.clone().oneshot(request.unwrap()).await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

async fn on_hand(pool: &PgPool, product: Uuid) -> String {
    sqlx::query_scalar("SELECT on_hand::text FROM stocks WHERE product_id = $1")
        .bind(product)
        .fetch_one(pool)
        .await
        .unwrap()
}

#[sqlx::test(migrations = false)]
async fn booking_with_products_needs_them_on_hand(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let mine = one(
        &pool,
        "INSERT INTO tenants (name) VALUES ('Salon') RETURNING id",
        Uuid::nil(),
    )
    .await;
    let client = one(&pool, "INSERT INTO clients (tenant_id, id, first_name, last_name, name) VALUES ($1, uuidv7(), 'Ada', 'L', 'Ada') RETURNING id", mine).await;
    let service = one(&pool, r#"INSERT INTO services (tenant_id, id, name, steps) VALUES ($1, uuidv7(), 'Hair colour', '[{"duration": "PT30M"}]') RETURNING id"#, mine).await;
    let room = one(
        &pool,
        "INSERT INTO resources (tenant_id, id, name) VALUES ($1, uuidv7(), 'Chair 1') RETURNING id",
        mine,
    )
    .await;
    one(&pool, r#"INSERT INTO schedules (tenant_id, id, name, recurrence, starts_at, ends_at) VALUES ($1, uuidv7(), 'Open', '"DTSTART:20261102T000000Z\nRRULE:FREQ=DAILY;COUNT=1"', '00:00', '23:00') RETURNING id"#, mine).await;
    let colour = one(&pool, "INSERT INTO products (tenant_id, id, sku, name, unit, price, currency) VALUES ($1, uuidv7(), 'WEL-60', 'Hair colour', 'tube', 12000000, 'USD') RETURNING id", mine).await;
    sqlx::query(
        "INSERT INTO stocks (tenant_id, id, product_id, on_hand) VALUES ($1, uuidv7(), $2, 1)",
    )
    .bind(mine)
    .bind(colour)
    .execute(&pool)
    .await
    .unwrap();
    let app = app(pool.clone(), mine);
    let appointments = format!("/api/v1/tenants/{mine}/appointments");
    let booking = |hour: &str, quantity: &str| {
        json!({
            "client_id": client, "service_id": service, "resource_id": room, "provider_ids": [],
            "starts_at": format!("2026-11-02T{hour}:00:00Z"), "ends_at": format!("2026-11-02T{hour}:30:00Z"),
            "status": "booked", "payment": {"payer_id": client, "method": "card"},
            "products": [{"product_id": colour, "quantity": quantity}]
        })
    };

    let (status, refused) = call(&app, "POST", &appointments, booking("09", "2")).await;
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "only one tube on hand: {refused}"
    );

    let (status, first) = call(&app, "POST", &appointments, booking("09", "1")).await;
    assert_eq!(status, StatusCode::OK, "{first}");
    assert_eq!(
        first["products"],
        json!([{"product_id": colour, "quantity": "1"}])
    );
    assert_eq!(
        on_hand(&pool, colour).await,
        "1",
        "booking takes nothing from stock"
    );

    let (status, second) = call(&app, "POST", &appointments, booking("10", "1")).await;
    assert_eq!(
        status,
        StatusCode::OK,
        "a booked appointment holds no stock: {second}"
    );

    let item = format!("{appointments}/{}", first["id"].as_str().unwrap());
    let more = json!({"products": [{"product_id": colour, "quantity": "3"}]});
    assert_eq!(
        call(&app, "PATCH", &item, more).await.0,
        StatusCode::CONFLICT
    );
    let (status, none) = call(&app, "PATCH", &item, json!({"products": []})).await;
    assert_eq!(
        (status, none["products"].clone()),
        (StatusCode::OK, json!([]))
    );

    let unknown = json!({"products": [{"product_id": Uuid::now_v7(), "quantity": "1"}]});
    assert_eq!(
        call(&app, "PATCH", &item, unknown).await.0,
        StatusCode::CONFLICT,
        "nothing of it is held"
    );
}
