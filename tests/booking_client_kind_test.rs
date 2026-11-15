//! Messaging (docs/TPD-messaging.md, docs/TPD-booking.md "Decided"): a client has a `kind`. The defined names are
//! `guest`, `client` and `prospect`, others are allowed, and changing a guest to a client is the conversion.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::middleware::from_fn;
use axum::{Extension, Router};
use megh::auth::rest::AppContext;
use megh::booking::client::ClientKind;
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

fn person(kind: Option<&str>) -> Value {
    let mut body = json!({"first_name": "Ada", "last_name": "Lovelace", "name": "Ada"});
    kind.into_iter().for_each(|kind| body["kind"] = json!(kind));
    body
}

#[sqlx::test(migrations = false)]
async fn a_client_created_without_a_kind_is_a_client(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let mine = tenant(&pool).await;
    let app = app(pool, mine);

    let (status, created) = call(
        &app,
        "POST",
        &format!("/api/v1/tenants/{mine}/clients"),
        person(None),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "{created}");
    assert_eq!(created["kind"], json!("client"));
}

#[sqlx::test(migrations = false)]
async fn the_column_defaults_to_client_for_a_row_inserted_without_one(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let mine = tenant(&pool).await;

    let kind: String = sqlx::query_scalar(
        "INSERT INTO clients (tenant_id, id, first_name, last_name, name) VALUES ($1, uuidv7(), 'Ada', 'Lovelace', 'Ada') RETURNING kind",
    )
    .bind(mine)
    .fetch_one(&pool)
    .await
    .unwrap();

    assert_eq!(kind, "client");
}

#[sqlx::test(migrations = false)]
async fn guest_client_prospect_and_an_open_name_round_trip(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let mine = tenant(&pool).await;
    let app = app(pool, mine);
    let collection = format!("/api/v1/tenants/{mine}/clients");

    for kind in ["guest", "client", "prospect", "vip"] {
        let (status, created) = call(&app, "POST", &collection, person(Some(kind))).await;
        assert_eq!(status, StatusCode::OK, "{created}");
        let (status, got) = call(
            &app,
            "GET",
            &format!("{collection}/{}", created["id"].as_str().unwrap()),
            Value::Null,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{got}");
        assert_eq!(
            (created["kind"].clone(), got["kind"].clone()),
            (json!(kind), json!(kind))
        );
    }
}

#[sqlx::test(migrations = false)]
async fn patching_a_guest_to_a_client_converts_it_and_keeps_the_row(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let mine = tenant(&pool).await;
    let app = app(pool, mine);
    let collection = format!("/api/v1/tenants/{mine}/clients");
    let (_, guest) = call(&app, "POST", &collection, person(Some("guest"))).await;
    let item = format!("{collection}/{}", guest["id"].as_str().unwrap());

    let (status, converted) = call(
        &app,
        "PATCH",
        &item,
        json!({"kind": "client", "first_name": "Ada", "email": "ada@example.com"}),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "{converted}");
    assert_eq!(
        (
            converted["id"].clone(),
            converted["kind"].clone(),
            converted["email"].clone()
        ),
        (
            guest["id"].clone(),
            json!("client"),
            json!("ada@example.com")
        )
    );
    let (_, got) = call(&app, "GET", &item, Value::Null).await;
    assert_eq!(got["kind"], json!("client"));
}

#[test]
fn the_defined_kinds_are_lower_case_text() {
    let guest = serde_json::to_string(&ClientKind::Guest).unwrap();
    let client = serde_json::to_string(&ClientKind::Client).unwrap();
    let prospect = serde_json::to_string(&ClientKind::Prospect).unwrap();

    assert_eq!(
        (guest.as_str(), client.as_str(), prospect.as_str()),
        ("\"guest\"", "\"client\"", "\"prospect\"")
    );
}

#[test]
fn an_open_name_reads_and_writes_back_as_the_same_text() {
    let kind: ClientKind = serde_json::from_str("\"vip\"").unwrap();

    assert_eq!(serde_json::to_string(&kind).unwrap(), "\"vip\"");
}

#[test]
fn a_defined_name_reads_as_its_kind() {
    let kind: ClientKind = serde_json::from_str("\"guest\"").unwrap();

    assert_eq!(kind, ClientKind::Guest);
}
