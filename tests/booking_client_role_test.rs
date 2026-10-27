//! S6a (docs/TPD-billing.md section 7, #227): a client has an optional role, as a provider has; a supplier is a
//! client with a role. Existing clients have none.

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

async fn tenant(pool: &PgPool) -> Uuid {
    sqlx::query_scalar("INSERT INTO tenants (name) VALUES ('Salon') RETURNING id")
        .fetch_one(pool)
        .await
        .unwrap()
}

fn app(pool: PgPool, tenant: Uuid) -> Router {
    let (api, _) = megh::booking::router().split_for_parts();
    let grants = vec![
        Grant::new(format!("tenants:read:{tenant}")),
        Grant::new("clients:*"),
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

fn names(page: &Value) -> Vec<String> {
    page["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["name"].as_str().unwrap().to_string())
        .collect()
}

#[sqlx::test(migrations = false)]
async fn a_client_has_an_optional_role(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let mine = tenant(&pool).await;
    let app = app(pool, mine);
    let collection = format!("/api/v1/tenants/{mine}/clients");

    let (status, wholesaler) = call(&app, "POST", &collection, json!({
        "first_name": "Colour", "last_name": "Wholesale", "name": "Colour Wholesale", "role": "supplier"
    }))
    .await;
    assert_eq!(status, StatusCode::OK, "{wholesaler}");
    assert_eq!(wholesaler["role"], json!("supplier"));

    let (status, ada) = call(
        &app,
        "POST",
        &collection,
        json!({"first_name": "Ada", "last_name": "Lovelace", "name": "Ada"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{ada}");
    assert_eq!(ada["role"], Value::Null, "no role unless given");

    let (status, empty) = call(
        &app,
        "POST",
        &collection,
        json!({"first_name": "A", "last_name": "B", "name": "C", "role": "  "}),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "a role is a name, never blank: {empty}"
    );

    let suppliers = format!("{collection}?filter=role%20%3D%20%22supplier%22");
    assert_eq!(
        names(&call(&app, "GET", &suppliers, Value::Null).await.1),
        ["Colour Wholesale"]
    );

    let item = format!("{collection}/{}", ada["id"].as_str().unwrap());
    let (status, patient) = call(&app, "PATCH", &item, json!({"role": "patient"})).await;
    assert_eq!(
        (status, patient["role"].clone()),
        (StatusCode::OK, json!("patient"))
    );
    let (status, cleared) = call(&app, "PATCH", &item, json!({"role": null})).await;
    assert_eq!(
        (status, cleared["role"].clone()),
        (StatusCode::OK, Value::Null)
    );
    let (status, got) = call(&app, "GET", &item, Value::Null).await;
    assert_eq!((status, got["role"].clone()), (StatusCode::OK, Value::Null));
}

#[sqlx::test(migrations = false)]
async fn existing_clients_have_no_role_after_the_migration(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let mine = tenant(&pool).await;
    sqlx::raw_sql("ALTER TABLE clients DROP COLUMN IF EXISTS role")
        .execute(&pool)
        .await
        .unwrap();
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO clients (tenant_id, id, first_name, last_name, name) VALUES ($1, uuidv7(), 'Old', 'Client', 'Old') RETURNING id",
    )
    .bind(mine)
    .fetch_one(&pool)
    .await
    .unwrap();

    megh::migrate(&pool).await.unwrap();
    let app = app(pool, mine);
    let (status, old) = call(
        &app,
        "GET",
        &format!("/api/v1/tenants/{mine}/clients/{id}"),
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{old}");
    assert_eq!(
        (old["name"].clone(), old["role"].clone()),
        (json!("Old"), Value::Null)
    );
}
