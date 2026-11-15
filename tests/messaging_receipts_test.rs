//! Messaging receipts (docs/TPD-messaging.md): `seen` and the unread count, and `typing`, which is published and
//! never stored.

use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::middleware::from_fn;
use axum::{Extension, Router};
use megh::auth::AccessToken;
use megh::auth::rest::AppContext;
use megh::messaging::{Content, Messaging, Party};
use megh::{Broker, Event, Grant, Topic, UserRepo, authorizer};
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
    ann: Uuid,
    ann_speaks: Uuid,
    bot_speaks: Uuid,
}

async fn chat(pool: &PgPool, messaging: &Messaging) -> Chat {
    let tenant = tenant(pool, "Acme").await;
    let ann = client(pool, tenant, "Ann").await;
    let bot = agent(pool, tenant, "assistant").await;
    let thread = messaging
        .open(tenant, Party::Client(ann), None)
        .await
        .unwrap()
        .id;
    let bot_speaks = messaging
        .join(tenant, thread, Party::Member(bot))
        .await
        .unwrap()
        .id;
    let ann_speaks = messaging.speaker(tenant, thread, ann).await.unwrap().id;
    Chat {
        tenant,
        thread,
        ann,
        ann_speaks,
        bot_speaks,
    }
}

async fn say(messaging: &Messaging, chat: &Chat, sender: Uuid, text: &str) -> Uuid {
    messaging
        .send(chat.tenant, chat.thread, sender, Content::text(text))
        .await
        .unwrap()
        .id
}

async fn last_read(pool: &PgPool, participant: Uuid) -> Option<Uuid> {
    sqlx::query_scalar("SELECT last_read_message_id FROM participants WHERE id = $1")
        .bind(participant)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn snapshot(pool: &PgPool) -> Vec<String> {
    sqlx::query_scalar(
        "SELECT 'threads ' || count(*) || ' ' || coalesce(max(last_message_at)::text, '') FROM threads
         UNION ALL SELECT 'participants ' || count(*) || ' ' || coalesce(max(last_read_message_id::text), '') FROM participants
         UNION ALL SELECT 'messages ' || count(*) FROM messages
         UNION ALL SELECT 'attachments ' || count(*) FROM attachments
         UNION ALL SELECT 'labels ' || count(*) FROM labels
         ORDER BY 1",
    )
    .fetch_all(pool)
    .await
    .unwrap()
}

#[sqlx::test(migrations = false)]
async fn seen_answers_how_many_messages_are_still_unread(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let (messaging, _) = messaging(&pool);
    let chat = chat(&pool, &messaging).await;
    let first = say(&messaging, &chat, chat.bot_speaks, "one").await;
    say(&messaging, &chat, chat.bot_speaks, "two").await;
    let third = say(&messaging, &chat, chat.bot_speaks, "three").await;

    let after_first = messaging
        .seen(chat.tenant, chat.thread, chat.ann_speaks, first)
        .await
        .unwrap();
    let after_third = messaging
        .seen(chat.tenant, chat.thread, chat.ann_speaks, third)
        .await
        .unwrap();

    assert_eq!((after_first, after_third), (2, 0));
}

#[sqlx::test(migrations = false)]
async fn seen_moves_the_participants_last_read_message(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let (messaging, _) = messaging(&pool);
    let chat = chat(&pool, &messaging).await;
    let first = say(&messaging, &chat, chat.bot_speaks, "one").await;
    let second = say(&messaging, &chat, chat.bot_speaks, "two").await;

    assert_eq!(last_read(&pool, chat.ann_speaks).await, None);
    messaging
        .seen(chat.tenant, chat.thread, chat.ann_speaks, first)
        .await
        .unwrap();
    assert_eq!(last_read(&pool, chat.ann_speaks).await, Some(first));
    messaging
        .seen(chat.tenant, chat.thread, chat.ann_speaks, second)
        .await
        .unwrap();
    assert_eq!(last_read(&pool, chat.ann_speaks).await, Some(second));
}

#[sqlx::test(migrations = false)]
async fn seen_does_not_move_another_participants_last_read_message(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let (messaging, _) = messaging(&pool);
    let chat = chat(&pool, &messaging).await;
    let first = say(&messaging, &chat, chat.bot_speaks, "one").await;

    messaging
        .seen(chat.tenant, chat.thread, chat.ann_speaks, first)
        .await
        .unwrap();

    assert_eq!(last_read(&pool, chat.bot_speaks).await, None);
}

#[sqlx::test(migrations = false)]
async fn a_participants_own_messages_are_never_unread_for_them(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let (messaging, _) = messaging(&pool);
    let chat = chat(&pool, &messaging).await;
    let first = say(&messaging, &chat, chat.bot_speaks, "one").await;
    say(&messaging, &chat, chat.ann_speaks, "mine").await;
    say(&messaging, &chat, chat.ann_speaks, "mine too").await;
    say(&messaging, &chat, chat.bot_speaks, "two").await;

    let left = messaging
        .seen(chat.tenant, chat.thread, chat.ann_speaks, first)
        .await
        .unwrap();

    assert_eq!(left, 1);
}

#[sqlx::test(migrations = false)]
async fn unread_lists_the_thread_where_the_other_party_spoke(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let (messaging, _) = messaging(&pool);
    let chat = chat(&pool, &messaging).await;
    say(&messaging, &chat, chat.bot_speaks, "Which service?").await;

    let unread = messaging
        .unread(chat.tenant, chat.ann_speaks)
        .await
        .unwrap();

    assert_eq!(unread.len(), 1);
}

#[sqlx::test(migrations = false)]
async fn seen_is_published_as_a_seen_event_on_the_thread_topic(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let (messaging, broker) = messaging(&pool);
    let chat = chat(&pool, &messaging).await;
    let first = say(&messaging, &chat, chat.bot_speaks, "one").await;
    let mut events = watch(
        &broker,
        &[format!("tenants/{}/threads/{}", chat.tenant, chat.thread)],
    )
    .await;

    messaging
        .seen(chat.tenant, chat.thread, chat.ann_speaks, first)
        .await
        .unwrap();

    let event = next(&mut events).await;
    assert_eq!(
        event.data,
        json!({"kind": "seen", "participant_id": chat.ann_speaks, "up_to": first})
    );
}

#[sqlx::test(migrations = false)]
async fn the_seen_route_marks_the_caller_as_having_read_up_to_a_message(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let (messaging, _) = messaging(&pool);
    let chat = chat(&pool, &messaging).await;
    let first = say(&messaging, &chat, chat.bot_speaks, "one").await;
    let ann_app = full_app(pool.clone(), messaging, chat.tenant, chat.ann);

    let (status, body) = call(
        &ann_app,
        "POST",
        &format!(
            "/api/v1/tenants/{}/threads/{}:seen",
            chat.tenant, chat.thread
        ),
        json!({"up_to": first}),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(last_read(&pool, chat.ann_speaks).await, Some(first));
}

#[sqlx::test(migrations = false)]
async fn the_seen_route_publishes_a_seen_event(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let (messaging, broker) = messaging(&pool);
    let chat = chat(&pool, &messaging).await;
    let first = say(&messaging, &chat, chat.bot_speaks, "one").await;
    let mut events = watch(
        &broker,
        &[format!("tenants/{}/threads/{}", chat.tenant, chat.thread)],
    )
    .await;
    let ann_app = full_app(pool, messaging, chat.tenant, chat.ann);

    call(
        &ann_app,
        "POST",
        &format!(
            "/api/v1/tenants/{}/threads/{}:seen",
            chat.tenant, chat.thread
        ),
        json!({"up_to": first}),
    )
    .await;

    let event = next(&mut events).await;
    assert_eq!(
        event.data,
        json!({"kind": "seen", "participant_id": chat.ann_speaks, "up_to": first})
    );
}

#[sqlx::test(migrations = false)]
async fn typing_is_published_as_a_typing_event_on_the_thread_topic(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let (messaging, broker) = messaging(&pool);
    let chat = chat(&pool, &messaging).await;
    let mut events = watch(
        &broker,
        &[format!("tenants/{}/threads/{}", chat.tenant, chat.thread)],
    )
    .await;

    messaging
        .typing(chat.tenant, chat.thread, chat.bot_speaks)
        .await
        .unwrap();

    let event = next(&mut events).await;
    assert_eq!(
        event.data,
        json!({"kind": "typing", "participant_id": chat.bot_speaks})
    );
}

#[sqlx::test(migrations = false)]
async fn typing_stores_nothing_in_any_table(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let (messaging, _) = messaging(&pool);
    let chat = chat(&pool, &messaging).await;
    let before = snapshot(&pool).await;

    messaging
        .typing(chat.tenant, chat.thread, chat.bot_speaks)
        .await
        .unwrap();

    assert_eq!(snapshot(&pool).await, before);
    let typing_tables: i64 = sqlx::query_scalar("SELECT count(*) FROM information_schema.tables WHERE table_schema = current_schema() AND table_name ILIKE '%typing%'")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(typing_tables, 0);
}

#[sqlx::test(migrations = false)]
async fn the_typing_route_publishes_a_typing_event_for_the_caller(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let (messaging, broker) = messaging(&pool);
    let chat = chat(&pool, &messaging).await;
    let mut events = watch(
        &broker,
        &[format!("tenants/{}/threads/{}", chat.tenant, chat.thread)],
    )
    .await;
    let ann_app = full_app(pool, messaging, chat.tenant, chat.ann);

    let (status, body) = call(
        &ann_app,
        "POST",
        &format!(
            "/api/v1/tenants/{}/threads/{}/typing",
            chat.tenant, chat.thread
        ),
        Value::Null,
    )
    .await;

    assert!(status.is_success(), "{status} {body}");
    let event = next(&mut events).await;
    assert_eq!(
        event.data,
        json!({"kind": "typing", "participant_id": chat.ann_speaks})
    );
}
