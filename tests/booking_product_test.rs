//! S6b (docs/TPD-billing.md section 7, #227): products and services have a current price and a unit; a ledger keeps
//! every price with the time it took effect; a tenant's stock of a product is read only.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::middleware::from_fn;
use axum::{Extension, Router};
use chrono::{DateTime, Duration, Utc};
use megh::auth::rest::AppContext;
use megh::{Grant, authorizer};
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::util::MapRequestLayer;
use tower::{Layer, ServiceExt};
use uuid::Uuid;

async fn tenant(pool: &PgPool, name: &str) -> Uuid {
    sqlx::query_scalar("INSERT INTO tenants (name) VALUES ($1) RETURNING id")
        .bind(name)
        .fetch_one(pool)
        .await
        .unwrap()
}

fn app(pool: PgPool, tenant: Uuid) -> Router {
    let (api, _) = megh::booking::router().split_for_parts();
    let grants = vec![
        Grant::new(format!("tenants:read:{tenant}")),
        Grant::new("products:*"),
        Grant::new("prices:*"),
        Grant::new("stocks:*"),
        Grant::new("services:*"),
    ];
    Router::new()
        .nest("/api/v1", api.layer(from_fn(authorizer)))
        .layer(Extension(grants))
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

fn items(page: &Value) -> Vec<Value> {
    page["items"].as_array().unwrap().clone()
}

fn colour(sku: &str) -> Value {
    json!({
        "sku": sku, "name": "Hair colour", "description": "Permanent, 60 ml", "unit": "tube",
        "price": 12000000, "currency": "USD", "attributes": {"brand": "Wella"}
    })
}

fn at(value: &Value) -> DateTime<Utc> {
    value.as_str().unwrap().parse().unwrap()
}

/// The ledger rows of one item, oldest first, as (price, currency).
async fn ledger(app: &Router, tenant: Uuid, item: &str) -> Vec<(Value, Value)> {
    let uri = format!("/api/v1/tenants/{tenant}/prices?filter=item_id%20%3D%20%22{item}%22");
    let (status, page) = call(app, "GET", &uri, Value::Null).await;
    assert_eq!(status, StatusCode::OK, "{page}");
    items(&page)
        .iter()
        .map(|row| (row["price"].clone(), row["currency"].clone()))
        .collect()
}

#[sqlx::test(migrations = false)]
async fn a_product_has_a_current_price_and_a_unit(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let mine = tenant(&pool, "Salon").await;
    let app = app(pool.clone(), mine);
    let products = format!("/api/v1/tenants/{mine}/products");

    let (status, product) = call(&app, "POST", &products, colour("WEL-60")).await;
    assert_eq!(status, StatusCode::OK, "{product}");
    assert_eq!(
        (
            product["sku"].clone(),
            product["unit"].clone(),
            product["price"].clone(),
            product["currency"].clone(),
            product["attributes"]["brand"].clone()
        ),
        (
            json!("WEL-60"),
            json!("tube"),
            json!(12000000),
            json!("USD"),
            json!("Wella")
        )
    );

    for (case, field) in [
        ("no price", "price"),
        ("no currency", "currency"),
        ("no unit", "unit"),
        ("no sku", "sku"),
    ] {
        let mut body = colour("OTHER");
        body.as_object_mut().unwrap().remove(field);
        assert_eq!(
            call(&app, "POST", &products, body).await.0,
            StatusCode::BAD_REQUEST,
            "{case}"
        );
    }
    let mut unknown = colour("OTHER");
    unknown["currency"] = json!("XYZ");
    assert_eq!(
        call(&app, "POST", &products, unknown).await.0,
        StatusCode::BAD_REQUEST,
        "an unknown currency"
    );

    assert_eq!(
        call(&app, "POST", &products, colour("WEL-60")).await.0,
        StatusCode::CONFLICT,
        "a SKU is unique per owner"
    );
    let theirs = tenant(&pool, "Other salon").await;
    let (status, same_sku) = call(
        &self::app(pool, theirs),
        "POST",
        &format!("/api/v1/tenants/{theirs}/products"),
        colour("WEL-60"),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "another owner may use the same SKU: {same_sku}"
    );

    let item = format!("{products}/{}", product["id"].as_str().unwrap());
    let (status, got) = call(&app, "GET", &item, Value::Null).await;
    assert_eq!(
        (status, got["price"].clone()),
        (StatusCode::OK, json!(12000000))
    );
    let (status, deleted) = call(&app, "DELETE", &item, Value::Null).await;
    assert_eq!(status, StatusCode::OK, "{deleted}");
    assert_eq!(
        call(&app, "GET", &item, Value::Null).await.0,
        StatusCode::NOT_FOUND
    );
}

#[sqlx::test(migrations = false)]
async fn the_ledger_keeps_every_price_with_when_it_took_effect(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let mine = tenant(&pool, "Salon").await;
    let app = app(pool, mine);
    let products = format!("/api/v1/tenants/{mine}/products");

    let before = Utc::now() - Duration::seconds(1);
    let (_, product) = call(&app, "POST", &products, colour("WEL-60")).await;
    let id = product["id"].as_str().unwrap();
    let item = format!("{products}/{id}");
    assert_eq!(
        ledger(&app, mine, id).await,
        [(json!(12000000), json!("USD"))],
        "created with its first price"
    );

    let (status, renamed) = call(&app, "PATCH", &item, json!({"name": "Hair colour, dark"})).await;
    assert_eq!(
        (status, renamed["name"].clone()),
        (StatusCode::OK, json!("Hair colour, dark"))
    );
    assert_eq!(
        ledger(&app, mine, id).await.len(),
        1,
        "no price change, no row"
    );

    let (status, dearer) = call(&app, "PATCH", &item, json!({"price": 13000000})).await;
    assert_eq!(
        (status, dearer["price"].clone()),
        (StatusCode::OK, json!(13000000))
    );
    let mut replaced = colour("WEL-60");
    replaced["price"] = json!(14000000);
    replaced["currency"] = json!("EUR");
    let (status, euros) = call(&app, "PUT", &item, replaced.clone()).await;
    assert_eq!(
        (status, euros["currency"].clone()),
        (StatusCode::OK, json!("EUR"))
    );
    let (status, _) = call(&app, "PUT", &item, replaced).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        ledger(&app, mine, id).await,
        [
            (json!(12000000), json!("USD")),
            (json!(13000000), json!("USD")),
            (json!(14000000), json!("EUR"))
        ],
        "a row per change, none for a PUT that keeps the price"
    );

    let prices = format!("/api/v1/tenants/{mine}/prices");
    let rows = items(
        &call(
            &app,
            "GET",
            &format!("{prices}?filter=item_id%20%3D%20%22{id}%22"),
            Value::Null,
        )
        .await
        .1,
    );
    assert!(rows.iter().all(|row| row["item_kind"] == json!("product")));
    let effective: Vec<DateTime<Utc>> = rows.iter().map(|row| at(&row["effective_at"])).collect();
    assert!(
        effective[0] >= before && effective.is_sorted(),
        "{effective:?}"
    );

    let row = format!("{prices}/{}", rows[1]["id"].as_str().unwrap());
    let (status, got) = call(&app, "GET", &row, Value::Null).await;
    assert_eq!(
        (status, got["price"].clone()),
        (StatusCode::OK, json!(13000000))
    );
    let written = json!({"item_id": id, "item_kind": "product", "price": 1, "currency": "USD"});
    assert_eq!(
        call(&app, "POST", &prices, written.clone()).await.0,
        StatusCode::METHOD_NOT_ALLOWED,
        "nobody writes the ledger directly"
    );
    for method in ["PUT", "PATCH", "DELETE"] {
        assert_eq!(
            call(&app, method, &row, written.clone()).await.0,
            StatusCode::METHOD_NOT_ALLOWED,
            "{method}: a ledger row is never changed"
        );
    }
}

#[sqlx::test(migrations = false)]
async fn a_service_has_a_current_price_and_a_unit(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let mine = tenant(&pool, "Salon").await;
    let app = app(pool.clone(), mine);
    let services = format!("/api/v1/tenants/{mine}/services");
    let cut = json!({"name": "Haircut", "steps": [{"duration": "PT30M"}]});

    assert_eq!(
        call(&app, "POST", &services, cut.clone()).await.0,
        StatusCode::BAD_REQUEST,
        "a service needs its price"
    );
    let mut priced = cut;
    priced["price"] = json!(40000000);
    priced["currency"] = json!("USD");
    let (status, service) = call(&app, "POST", &services, priced).await;
    assert_eq!(status, StatusCode::OK, "{service}");
    assert_eq!(
        (
            service["price"].clone(),
            service["currency"].clone(),
            service["unit"].clone()
        ),
        (json!(40000000), json!("USD"), json!("session"))
    );
    let id = service["id"].as_str().unwrap();
    let (status, _) = call(
        &app,
        "PATCH",
        &format!("{services}/{id}"),
        json!({"price": 45000000}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        ledger(&app, mine, id).await,
        [
            (json!(40000000), json!("USD")),
            (json!(45000000), json!("USD"))
        ]
    );

    let existing: Uuid = sqlx::query_scalar(
        r#"INSERT INTO services (tenant_id, id, name, steps) VALUES ($1, uuidv7(), 'Old', '[{"duration": "PT30M"}]') RETURNING id"#,
    )
    .bind(mine)
    .fetch_one(&pool)
    .await
    .unwrap();
    let (status, old) = call(&app, "GET", &format!("{services}/{existing}"), Value::Null).await;
    assert_eq!(status, StatusCode::OK, "{old}");
    assert_eq!(
        (
            old["price"].clone(),
            old["currency"].clone(),
            old["unit"].clone()
        ),
        (json!(0), json!("USD"), json!("session")),
        "a service stored before prices"
    );
    assert!(
        ledger(&app, mine, &existing.to_string()).await.is_empty(),
        "no row until its price changes"
    );
}

#[sqlx::test(migrations = false)]
async fn stock_is_read_only_and_a_tenant_may_stock_another_tenants_product(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let brand = tenant(&pool, "Brand").await;
    let location = tenant(&pool, "Location").await;
    let (_, product) = call(
        &app(pool.clone(), brand),
        "POST",
        &format!("/api/v1/tenants/{brand}/products"),
        colour("WEL-60"),
    )
    .await;
    let product_id: Uuid = product["id"].as_str().unwrap().parse().unwrap();
    let app = app(pool.clone(), location);
    let stocks = format!("/api/v1/tenants/{location}/stocks");

    assert!(items(&call(&app, "GET", &stocks, Value::Null).await.1).is_empty());
    assert_eq!(
        call(
            &app,
            "GET",
            &format!("{stocks}/{}", Uuid::now_v7()),
            Value::Null
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );

    let stock_id: Uuid = sqlx::query_scalar(
        "INSERT INTO stocks (tenant_id, id, product_id, on_hand) VALUES ($1, uuidv7(), $2, 4) RETURNING id",
    )
    .bind(location)
    .bind(product_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    let (status, by_id) = call(&app, "GET", &format!("{stocks}/{stock_id}"), Value::Null).await;
    assert_eq!(status, StatusCode::OK, "{by_id}");
    assert_eq!(
        (by_id["product_id"].clone(), by_id["on_hand"].clone()),
        (json!(product_id), json!("4"))
    );

    let of_product = format!("/api/v1/tenants/{location}/products/{product_id}:stock");
    let (status, held) = call(&app, "GET", &of_product, Value::Null).await;
    assert_eq!(status, StatusCode::OK, "{held}");
    assert_eq!(
        (held["id"].clone(), held["on_hand"].clone()),
        (json!(stock_id), json!("4"))
    );

    let body = json!({"product_id": product_id, "on_hand": "9"});
    for (method, uri) in [
        ("POST", stocks.clone()),
        ("PUT", format!("{stocks}/{stock_id}")),
        ("PATCH", format!("{stocks}/{stock_id}")),
        ("DELETE", format!("{stocks}/{stock_id}")),
    ] {
        assert_eq!(
            call(&app, method, &uri, body.clone()).await.0,
            StatusCode::METHOD_NOT_ALLOWED,
            "{method} {uri}: stock is read only"
        );
    }
    let twice = sqlx::query(
        "INSERT INTO stocks (tenant_id, id, product_id, on_hand) VALUES ($1, uuidv7(), $2, 1)",
    )
    .bind(location)
    .bind(product_id)
    .execute(&pool)
    .await;
    assert!(twice.is_err(), "one stock row per tenant and product");
}
