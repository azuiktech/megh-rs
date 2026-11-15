//! Messaging attachments (docs/TPD-messaging.md): a raw upload, attached to a message by id, downloaded with its content
//! type, capped at 10 MiB unless the app says otherwise, and belonging to its thread and tenant.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::middleware::from_fn;
use axum::{Extension, Router};
use megh::auth::AccessToken;
use megh::auth::rest::AppContext;
use megh::messaging::{Messaging, Party};
use megh::{Broker, Grant, UserRepo, authorizer};
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

async fn client(pool: &PgPool, tenant: Uuid, name: &str) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO clients (tenant_id, id, first_name, last_name, name) VALUES ($1, uuidv7(), $2, 'X', $2) RETURNING id",
    )
    .bind(tenant)
    .bind(name)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn agent(pool: &PgPool, tenant: Uuid, name: &str) -> Uuid {
    UserRepo::new(pool)
        .machine(tenant, name, vec![format!("tenants:read:{tenant}")])
        .await
        .unwrap()
        .id
}

fn token(sub: Uuid, tenant: Uuid) -> AccessToken {
    AccessToken {
        sub,
        iss: "megh".into(),
        aud: vec![tenant.to_string()],
        exp: u64::MAX,
        iat: 0,
        client_id: "megh".into(),
        scope: String::new(),
    }
}

fn messaging(pool: &PgPool) -> (Messaging, Broker) {
    let broker = Broker::new();
    (Messaging::new(pool.clone(), broker.clone()), broker)
}

fn app(pool: PgPool, messaging: Messaging, tenant: Uuid, sub: Uuid, grants: &[&str]) -> Router {
    let (api, _) = megh::messaging::router().split_for_parts();
    let grants = grants
        .iter()
        .map(|grant| Grant::new(*grant))
        .collect::<Vec<_>>();
    Router::new()
        .nest("/api/v1", api.layer(from_fn(authorizer)))
        .layer(Extension(grants))
        .layer(Extension(token(sub, tenant)))
        .layer(Extension(messaging))
        .with_state(AppContext { pool })
}

fn everything(tenant: Uuid) -> Vec<String> {
    let mut grants = vec![format!("tenants:read:{tenant}")];
    grants.extend(
        [
            "threads:*",
            "participants:*",
            "messages:*",
            "labels:*",
            "typing:*",
            "attachments:*",
        ]
        .map(String::from),
    );
    grants
}

fn full_app(pool: PgPool, messaging: Messaging, tenant: Uuid, sub: Uuid) -> Router {
    let grants = everything(tenant);
    app(
        pool,
        messaging,
        tenant,
        sub,
        &grants.iter().map(String::as_str).collect::<Vec<_>>(),
    )
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

const TEN_MIB: usize = 10 * 1024 * 1024;

async fn raw(
    app: &Router,
    method: &str,
    uri: &str,
    content_type: Option<&str>,
    body: Vec<u8>,
) -> (StatusCode, Option<String>, Vec<u8>) {
    let request = Request::builder().method(method).uri(uri);
    let request = match content_type {
        Some(content_type) => request.header("content-type", content_type),
        None => request,
    };
    let response = app
        .clone()
        .oneshot(request.body(Body::from(body)).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let content_type = response
        .headers()
        .get("content-type")
        .map(|value| value.to_str().unwrap().to_string());
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, content_type, bytes.to_vec())
}

struct Chat {
    tenant: Uuid,
    thread: Uuid,
    ann: Uuid,
}

async fn chat(pool: &PgPool, messaging: &Messaging) -> Chat {
    let tenant = tenant(pool, "Acme").await;
    let ann = client(pool, tenant, "Ann").await;
    let thread = messaging
        .open(tenant, Party::Client(ann), None)
        .await
        .unwrap()
        .id;
    Chat {
        tenant,
        thread,
        ann,
    }
}

fn upload_uri(tenant: Uuid, thread: Uuid, filename: &str) -> String {
    format!("/api/v1/tenants/{tenant}/threads/{thread}/attachments?filename={filename}")
}

async fn upload(
    app: &Router,
    chat: &Chat,
    filename: &str,
    content_type: &str,
    bytes: Vec<u8>,
) -> (StatusCode, Value) {
    let (status, _, body) = raw(
        app,
        "POST",
        &upload_uri(chat.tenant, chat.thread, filename),
        Some(content_type),
        bytes,
    )
    .await;
    (status, serde_json::from_slice(&body).unwrap_or(Value::Null))
}

async fn stored(pool: &PgPool) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM attachments")
        .fetch_one(pool)
        .await
        .unwrap()
}

#[sqlx::test(migrations = false)]
async fn an_upload_answers_the_metadata_of_the_stored_file(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let (messaging, _) = messaging(&pool);
    let chat = chat(&pool, &messaging).await;
    let app = full_app(pool, messaging, chat.tenant, chat.ann);

    let (status, attachment) = upload(
        &app,
        &chat,
        "price-list.txt",
        "text/plain",
        b"hello".to_vec(),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "{attachment}");
    assert_eq!(
        (
            attachment["filename"].clone(),
            attachment["content_type"].clone(),
            attachment["size"].clone()
        ),
        (json!("price-list.txt"), json!("text/plain"), json!(5))
    );
    assert!(
        attachment["id"]
            .as_str()
            .is_some_and(|id| Uuid::try_parse(id).is_ok())
    );
}

#[sqlx::test(migrations = false)]
async fn a_message_lists_the_attachments_it_was_sent_with(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let (messaging, _) = messaging(&pool);
    let chat = chat(&pool, &messaging).await;
    let app = full_app(pool, messaging, chat.tenant, chat.ann);
    let (_, attachment) = upload(
        &app,
        &chat,
        "price-list.txt",
        "text/plain",
        b"hello".to_vec(),
    )
    .await;

    let (status, message) = call(
        &app,
        "POST",
        &format!(
            "/api/v1/tenants/{}/threads/{}/messages",
            chat.tenant, chat.thread
        ),
        json!({"text": "See this", "attachments": [attachment["id"]]}),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "{message}");
    assert_eq!(
        message["attachments"],
        json!([{"id": attachment["id"], "filename": "price-list.txt", "content_type": "text/plain", "size": 5}])
    );
}

#[sqlx::test(migrations = false)]
async fn the_listed_messages_carry_their_attachments_too(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let (messaging, _) = messaging(&pool);
    let chat = chat(&pool, &messaging).await;
    let app = full_app(pool, messaging, chat.tenant, chat.ann);
    let (_, attachment) = upload(
        &app,
        &chat,
        "price-list.txt",
        "text/plain",
        b"hello".to_vec(),
    )
    .await;
    call(
        &app,
        "POST",
        &format!(
            "/api/v1/tenants/{}/threads/{}/messages",
            chat.tenant, chat.thread
        ),
        json!({"text": "See this", "attachments": [attachment["id"]]}),
    )
    .await;

    let (_, page) = call(
        &app,
        "GET",
        &format!(
            "/api/v1/tenants/{}/threads/{}/messages",
            chat.tenant, chat.thread
        ),
        Value::Null,
    )
    .await;

    assert_eq!(page["items"][0]["attachments"][0]["id"], attachment["id"]);
}

#[sqlx::test(migrations = false)]
async fn a_download_answers_the_bytes_with_the_uploaded_content_type(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let (messaging, _) = messaging(&pool);
    let chat = chat(&pool, &messaging).await;
    let app = full_app(pool, messaging, chat.tenant, chat.ann);
    let bytes = vec![0u8, 255, 1, 128, 10];
    let (_, attachment) = upload(&app, &chat, "pixel.png", "image/png", bytes.clone()).await;

    let (status, content_type, body) = raw(
        &app,
        "GET",
        &format!(
            "/api/v1/tenants/{}/threads/{}/attachments/{}",
            chat.tenant,
            chat.thread,
            attachment["id"].as_str().unwrap()
        ),
        None,
        Vec::new(),
    )
    .await;

    assert_eq!(
        (status, content_type.as_deref(), body),
        (StatusCode::OK, Some("image/png"), bytes)
    );
}

#[sqlx::test(migrations = false)]
async fn an_upload_of_exactly_ten_mebibytes_is_accepted_by_default(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let (messaging, _) = messaging(&pool);
    let chat = chat(&pool, &messaging).await;
    let app = full_app(pool, messaging, chat.tenant, chat.ann);

    let (status, attachment) = upload(
        &app,
        &chat,
        "big.bin",
        "application/octet-stream",
        vec![7; TEN_MIB],
    )
    .await;

    assert_eq!(
        (status, attachment["size"].clone()),
        (StatusCode::OK, json!(TEN_MIB))
    );
}

#[sqlx::test(migrations = false)]
async fn an_upload_over_ten_mebibytes_is_refused_with_413_by_default(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let (messaging, _) = messaging(&pool);
    let chat = chat(&pool, &messaging).await;
    let app = full_app(pool.clone(), messaging, chat.tenant, chat.ann);

    let (status, _) = upload(
        &app,
        &chat,
        "big.bin",
        "application/octet-stream",
        vec![7; TEN_MIB + 1],
    )
    .await;

    assert_eq!(
        (status, stored(&pool).await),
        (StatusCode::PAYLOAD_TOO_LARGE, 0)
    );
}

#[sqlx::test(migrations = false)]
async fn the_limit_set_with_attachment_limit_replaces_the_default(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let broker = Broker::new();
    let messaging = Messaging::new(pool.clone(), broker).with_attachment_limit(8);
    let chat = chat(&pool, &messaging).await;
    let app = full_app(pool.clone(), messaging, chat.tenant, chat.ann);

    let (at_limit, _) = upload(&app, &chat, "a.bin", "application/octet-stream", vec![1; 8]).await;
    let (over_limit, _) =
        upload(&app, &chat, "b.bin", "application/octet-stream", vec![1; 9]).await;

    assert_eq!(
        (at_limit, over_limit, stored(&pool).await),
        (StatusCode::OK, StatusCode::PAYLOAD_TOO_LARGE, 1)
    );
}

#[sqlx::test(migrations = false)]
async fn an_attachment_is_not_served_under_another_thread(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let (messaging, _) = messaging(&pool);
    let chat = chat(&pool, &messaging).await;
    let bob = client(&pool, chat.tenant, "Bob").await;
    let other_thread = messaging
        .open(chat.tenant, Party::Client(bob), None)
        .await
        .unwrap()
        .id;
    let app = full_app(pool, messaging, chat.tenant, chat.ann);
    let (_, attachment) = upload(&app, &chat, "a.txt", "text/plain", b"hello".to_vec()).await;

    let (status, _, _) = raw(
        &app,
        "GET",
        &format!(
            "/api/v1/tenants/{}/threads/{other_thread}/attachments/{}",
            chat.tenant,
            attachment["id"].as_str().unwrap()
        ),
        None,
        Vec::new(),
    )
    .await;

    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[sqlx::test(migrations = false)]
async fn an_attachment_is_not_served_under_another_tenant(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let (messaging, _) = messaging(&pool);
    let chat = chat(&pool, &messaging).await;
    let globex = tenant(&pool, "Globex").await;
    let app = full_app(pool.clone(), messaging.clone(), chat.tenant, chat.ann);
    let globex_app = full_app(pool, messaging, globex, Uuid::now_v7());
    let (_, attachment) = upload(&app, &chat, "a.txt", "text/plain", b"hello".to_vec()).await;

    let (status, _, _) = raw(
        &globex_app,
        "GET",
        &format!(
            "/api/v1/tenants/{globex}/threads/{}/attachments/{}",
            chat.thread,
            attachment["id"].as_str().unwrap()
        ),
        None,
        Vec::new(),
    )
    .await;

    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[sqlx::test(migrations = false)]
async fn a_message_cannot_carry_an_attachment_of_another_thread(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let (messaging, _) = messaging(&pool);
    let chat = chat(&pool, &messaging).await;
    let bob = client(&pool, chat.tenant, "Bob").await;
    let other_thread = messaging
        .open(chat.tenant, Party::Client(bob), None)
        .await
        .unwrap()
        .id;
    let bot = agent(&pool, chat.tenant, "assistant").await;
    let app = full_app(pool.clone(), messaging.clone(), chat.tenant, chat.ann);
    let bot_app = full_app(pool, messaging, chat.tenant, bot);
    let (_, attachment) = upload(&app, &chat, "a.txt", "text/plain", b"hello".to_vec()).await;

    let (status, _) = call(
        &bot_app,
        "POST",
        &format!(
            "/api/v1/tenants/{}/threads/{other_thread}/messages",
            chat.tenant
        ),
        json!({"text": "Mine now", "attachments": [attachment["id"]]}),
    )
    .await;

    assert!(status.is_client_error(), "{status}");
}
