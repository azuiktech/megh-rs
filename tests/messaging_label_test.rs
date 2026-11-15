//! Messaging labels (docs/TPD-messaging.md): tenant labels on a thread and on a message, idempotent, announced with a
//! `labels` event, and never from another tenant.

use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::middleware::from_fn;
use axum::{Extension, Router};
use megh::auth::AccessToken;
use megh::auth::rest::AppContext;
use megh::messaging::{Content, Labelled, Messaging, Party};
use megh::{Broker, Event, Grant, Topic, authorizer};
use serde_json::{Value, json};
use sqlx::PgPool;
use tokio::sync::mpsc::Receiver;
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

async fn watch(broker: &Broker, topics: &[String]) -> Receiver<Event> {
    let receiver = broker.register("watcher", None).await;
    let topics = topics
        .iter()
        .map(|topic| Topic::new(topic.as_str()))
        .collect::<Vec<_>>();
    broker.subscribe("watcher", &topics).await.unwrap();
    receiver
}

async fn next(receiver: &mut Receiver<Event>) -> Event {
    tokio::time::timeout(Duration::from_secs(2), receiver.recv())
        .await
        .expect("an event arrives")
        .expect("the stream is open")
}

struct Chat {
    tenant: Uuid,
    thread: Uuid,
    message: Uuid,
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
    let speaker = messaging.speaker(tenant, thread, ann).await.unwrap().id;
    let message = messaging
        .send(tenant, thread, speaker, Content::text("Hi"))
        .await
        .unwrap()
        .id;
    Chat {
        tenant,
        thread,
        message,
        ann,
    }
}

async fn label(pool: &PgPool, tenant: Uuid, name: &str) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO labels (tenant_id, id, name) VALUES ($1, uuidv7(), $2) RETURNING id",
    )
    .bind(tenant)
    .bind(name)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn on_thread(pool: &PgPool, thread: Uuid) -> Vec<Uuid> {
    sqlx::query_scalar("SELECT label_id FROM thread_labels WHERE thread_id = $1 ORDER BY label_id")
        .bind(thread)
        .fetch_all(pool)
        .await
        .unwrap()
}

async fn on_message(pool: &PgPool, message: Uuid) -> Vec<Uuid> {
    sqlx::query_scalar(
        "SELECT label_id FROM message_labels WHERE message_id = $1 ORDER BY label_id",
    )
    .bind(message)
    .fetch_all(pool)
    .await
    .unwrap()
}

fn labels_uri(tenant: Uuid) -> String {
    format!("/api/v1/tenants/{tenant}/labels")
}

#[sqlx::test(migrations = false)]
async fn a_label_is_created_with_a_name_and_an_optional_colour(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let (messaging, _) = messaging(&pool);
    let chat = chat(&pool, &messaging).await;
    let app = full_app(pool, messaging, chat.tenant, chat.ann);

    let (status, created) = call(
        &app,
        "POST",
        &labels_uri(chat.tenant),
        json!({"name": "urgent", "colour": "#ff0000"}),
    )
    .await;
    let (plain_status, plain) = call(
        &app,
        "POST",
        &labels_uri(chat.tenant),
        json!({"name": "vip"}),
    )
    .await;

    assert_eq!(
        (status, plain_status),
        (StatusCode::OK, StatusCode::OK),
        "{created} {plain}"
    );
    assert_eq!(
        (
            created["name"].clone(),
            created["colour"].clone(),
            plain["colour"].clone()
        ),
        (json!("urgent"), json!("#ff0000"), Value::Null)
    );
}

#[sqlx::test(migrations = false)]
async fn labels_are_listed_for_their_own_tenant_only(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let (messaging, _) = messaging(&pool);
    let chat = chat(&pool, &messaging).await;
    let globex = tenant(&pool, "Globex").await;
    label(&pool, chat.tenant, "urgent").await;
    label(&pool, globex, "secret").await;
    let app = full_app(pool, messaging, chat.tenant, chat.ann);

    let (status, page) = call(&app, "GET", &labels_uri(chat.tenant), Value::Null).await;

    assert_eq!(status, StatusCode::OK, "{page}");
    let names = page["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["name"].clone())
        .collect::<Vec<_>>();
    assert_eq!(names, [json!("urgent")]);
}

#[sqlx::test(migrations = false)]
async fn a_label_is_renamed_by_patch_and_read_back(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let (messaging, _) = messaging(&pool);
    let chat = chat(&pool, &messaging).await;
    let urgent = label(&pool, chat.tenant, "urgent").await;
    let app = full_app(pool, messaging, chat.tenant, chat.ann);
    let item = format!("{}/{urgent}", labels_uri(chat.tenant));

    let (status, patched) = call(&app, "PATCH", &item, json!({"name": "asap"})).await;
    let (_, got) = call(&app, "GET", &item, Value::Null).await;

    assert_eq!(status, StatusCode::OK, "{patched}");
    assert_eq!(got["name"], json!("asap"));
}

#[sqlx::test(migrations = false)]
async fn a_deleted_label_is_gone(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let (messaging, _) = messaging(&pool);
    let chat = chat(&pool, &messaging).await;
    let urgent = label(&pool, chat.tenant, "urgent").await;
    let app = full_app(pool, messaging, chat.tenant, chat.ann);
    let item = format!("{}/{urgent}", labels_uri(chat.tenant));

    let (deleted, _) = call(&app, "DELETE", &item, Value::Null).await;
    let (after, _) = call(&app, "GET", &item, Value::Null).await;

    assert_eq!((deleted.is_success(), after), (true, StatusCode::NOT_FOUND));
}

#[sqlx::test(migrations = false)]
async fn a_label_cannot_be_read_by_another_tenant(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let (messaging, _) = messaging(&pool);
    let chat = chat(&pool, &messaging).await;
    let globex = tenant(&pool, "Globex").await;
    let urgent = label(&pool, chat.tenant, "urgent").await;
    let globex_app = full_app(pool, messaging, globex, Uuid::now_v7());

    let (status, _) = call(
        &globex_app,
        "GET",
        &format!("{}/{urgent}", labels_uri(globex)),
        Value::Null,
    )
    .await;

    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[sqlx::test(migrations = false)]
async fn a_thread_is_labelled_through_the_interface(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let (messaging, _) = messaging(&pool);
    let chat = chat(&pool, &messaging).await;
    let urgent = label(&pool, chat.tenant, "urgent").await;

    messaging
        .label(chat.tenant, Labelled::Thread(chat.thread), urgent)
        .await
        .unwrap();

    assert_eq!(
        (
            on_thread(&pool, chat.thread).await,
            on_message(&pool, chat.message).await
        ),
        (vec![urgent], vec![])
    );
}

#[sqlx::test(migrations = false)]
async fn a_message_is_labelled_through_the_interface(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let (messaging, _) = messaging(&pool);
    let chat = chat(&pool, &messaging).await;
    let urgent = label(&pool, chat.tenant, "urgent").await;

    messaging
        .label(chat.tenant, Labelled::Message(chat.message), urgent)
        .await
        .unwrap();

    assert_eq!(
        (
            on_message(&pool, chat.message).await,
            on_thread(&pool, chat.thread).await
        ),
        (vec![urgent], vec![])
    );
}

#[sqlx::test(migrations = false)]
async fn unlabelling_takes_the_label_off_and_leaves_the_others(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let (messaging, _) = messaging(&pool);
    let chat = chat(&pool, &messaging).await;
    let (urgent, vip) = (
        label(&pool, chat.tenant, "urgent").await,
        label(&pool, chat.tenant, "vip").await,
    );
    messaging
        .label(chat.tenant, Labelled::Thread(chat.thread), urgent)
        .await
        .unwrap();
    messaging
        .label(chat.tenant, Labelled::Thread(chat.thread), vip)
        .await
        .unwrap();

    messaging
        .unlabel(chat.tenant, Labelled::Thread(chat.thread), urgent)
        .await
        .unwrap();

    assert_eq!(on_thread(&pool, chat.thread).await, vec![vip]);
}

#[sqlx::test(migrations = false)]
async fn unlabelling_a_message_takes_the_label_off(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let (messaging, _) = messaging(&pool);
    let chat = chat(&pool, &messaging).await;
    let urgent = label(&pool, chat.tenant, "urgent").await;
    messaging
        .label(chat.tenant, Labelled::Message(chat.message), urgent)
        .await
        .unwrap();

    messaging
        .unlabel(chat.tenant, Labelled::Message(chat.message), urgent)
        .await
        .unwrap();

    assert_eq!(on_message(&pool, chat.message).await, Vec::<Uuid>::new());
}

#[sqlx::test(migrations = false)]
async fn labelling_twice_is_idempotent(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let (messaging, _) = messaging(&pool);
    let chat = chat(&pool, &messaging).await;
    let urgent = label(&pool, chat.tenant, "urgent").await;

    let first = messaging
        .label(chat.tenant, Labelled::Thread(chat.thread), urgent)
        .await;
    let second = messaging
        .label(chat.tenant, Labelled::Thread(chat.thread), urgent)
        .await;
    let message_first = messaging
        .label(chat.tenant, Labelled::Message(chat.message), urgent)
        .await;
    let message_second = messaging
        .label(chat.tenant, Labelled::Message(chat.message), urgent)
        .await;

    assert!(first.is_ok() && second.is_ok() && message_first.is_ok() && message_second.is_ok());
    assert_eq!(
        (
            on_thread(&pool, chat.thread).await,
            on_message(&pool, chat.message).await
        ),
        (vec![urgent], vec![urgent])
    );
}

#[sqlx::test(migrations = false)]
async fn labelling_a_thread_publishes_a_labels_event_with_the_current_labels(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let (messaging, broker) = messaging(&pool);
    let chat = chat(&pool, &messaging).await;
    let urgent = label(&pool, chat.tenant, "urgent").await;
    let mut events = watch(
        &broker,
        &[format!("tenants/{}/threads/{}", chat.tenant, chat.thread)],
    )
    .await;

    messaging
        .label(chat.tenant, Labelled::Thread(chat.thread), urgent)
        .await
        .unwrap();

    let event = next(&mut events).await;
    assert_eq!(
        event.data,
        json!({"kind": "labels", "target": {"thread": chat.thread}, "labels": [urgent]})
    );
}

#[sqlx::test(migrations = false)]
async fn labelling_a_message_publishes_a_labels_event_on_its_threads_topic(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let (messaging, broker) = messaging(&pool);
    let chat = chat(&pool, &messaging).await;
    let urgent = label(&pool, chat.tenant, "urgent").await;
    let mut events = watch(
        &broker,
        &[format!("tenants/{}/threads/{}", chat.tenant, chat.thread)],
    )
    .await;

    messaging
        .label(chat.tenant, Labelled::Message(chat.message), urgent)
        .await
        .unwrap();

    let event = next(&mut events).await;
    assert_eq!(
        event.data,
        json!({"kind": "labels", "target": {"message": chat.message}, "labels": [urgent]})
    );
}

#[sqlx::test(migrations = false)]
async fn unlabelling_publishes_a_labels_event_without_that_label(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let (messaging, broker) = messaging(&pool);
    let chat = chat(&pool, &messaging).await;
    let urgent = label(&pool, chat.tenant, "urgent").await;
    messaging
        .label(chat.tenant, Labelled::Thread(chat.thread), urgent)
        .await
        .unwrap();
    let mut events = watch(
        &broker,
        &[format!("tenants/{}/threads/{}", chat.tenant, chat.thread)],
    )
    .await;

    messaging
        .unlabel(chat.tenant, Labelled::Thread(chat.thread), urgent)
        .await
        .unwrap();

    let event = next(&mut events).await;
    assert_eq!(
        event.data,
        json!({"kind": "labels", "target": {"thread": chat.thread}, "labels": []})
    );
}

#[sqlx::test(migrations = false)]
async fn a_label_of_another_tenant_is_refused_and_not_attached(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let (messaging, _) = messaging(&pool);
    let chat = chat(&pool, &messaging).await;
    let globex = tenant(&pool, "Globex").await;
    let foreign = label(&pool, globex, "secret").await;

    let thread = messaging
        .label(chat.tenant, Labelled::Thread(chat.thread), foreign)
        .await;
    let message = messaging
        .label(chat.tenant, Labelled::Message(chat.message), foreign)
        .await;

    assert!(thread.is_err() && message.is_err());
    assert_eq!(
        (
            on_thread(&pool, chat.thread).await,
            on_message(&pool, chat.message).await
        ),
        (vec![], vec![])
    );
}

#[sqlx::test(migrations = false)]
async fn a_thread_of_another_tenant_cannot_be_labelled(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let (messaging, _) = messaging(&pool);
    let chat = chat(&pool, &messaging).await;
    let globex = tenant(&pool, "Globex").await;
    let own = label(&pool, globex, "mine").await;

    let result = messaging
        .label(globex, Labelled::Thread(chat.thread), own)
        .await;

    assert!(result.is_err());
    assert_eq!(on_thread(&pool, chat.thread).await, Vec::<Uuid>::new());
}

#[sqlx::test(migrations = false)]
async fn the_thread_label_routes_put_and_delete_a_label(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let (messaging, _) = messaging(&pool);
    let chat = chat(&pool, &messaging).await;
    let urgent = label(&pool, chat.tenant, "urgent").await;
    let app = full_app(pool.clone(), messaging, chat.tenant, chat.ann);
    let uri = format!(
        "/api/v1/tenants/{}/threads/{}/labels/{urgent}",
        chat.tenant, chat.thread
    );

    let (put, put_body) = call(&app, "PUT", &uri, Value::Null).await;
    let labelled = on_thread(&pool, chat.thread).await;
    let (delete, delete_body) = call(&app, "DELETE", &uri, Value::Null).await;

    assert!(
        put.is_success() && delete.is_success(),
        "{put} {put_body} {delete} {delete_body}"
    );
    assert_eq!(
        (labelled, on_thread(&pool, chat.thread).await),
        (vec![urgent], vec![])
    );
}

#[sqlx::test(migrations = false)]
async fn the_message_label_routes_put_and_delete_a_label(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let (messaging, _) = messaging(&pool);
    let chat = chat(&pool, &messaging).await;
    let urgent = label(&pool, chat.tenant, "urgent").await;
    let app = full_app(pool.clone(), messaging, chat.tenant, chat.ann);
    let uri = format!(
        "/api/v1/tenants/{}/threads/{}/messages/{}/labels/{urgent}",
        chat.tenant, chat.thread, chat.message
    );

    let (put, put_body) = call(&app, "PUT", &uri, Value::Null).await;
    let labelled = on_message(&pool, chat.message).await;
    let (delete, delete_body) = call(&app, "DELETE", &uri, Value::Null).await;

    assert!(
        put.is_success() && delete.is_success(),
        "{put} {put_body} {delete} {delete_body}"
    );
    assert_eq!(
        (labelled, on_message(&pool, chat.message).await),
        (vec![urgent], vec![])
    );
}

#[sqlx::test(migrations = false)]
async fn the_label_route_refuses_a_label_of_another_tenant(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let (messaging, _) = messaging(&pool);
    let chat = chat(&pool, &messaging).await;
    let globex = tenant(&pool, "Globex").await;
    let foreign = label(&pool, globex, "secret").await;
    let app = full_app(pool.clone(), messaging, chat.tenant, chat.ann);

    let (status, _) = call(
        &app,
        "PUT",
        &format!(
            "/api/v1/tenants/{}/threads/{}/labels/{foreign}",
            chat.tenant, chat.thread
        ),
        Value::Null,
    )
    .await;

    assert!(status.is_client_error(), "{status}");
    assert_eq!(on_thread(&pool, chat.thread).await, Vec::<Uuid>::new());
}
