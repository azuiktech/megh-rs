//! Client identity (PRD 11.8): a token for a known client, a guest from the public route kept in the session, the
//! guest's chat moved to the client it turns out to be, and the broker letting a client follow only its own topics.

use std::time::Duration;

use axum::body::{Body, to_bytes};
use axum::http::{Request, Response, StatusCode, header};
use axum::middleware::{from_fn, from_fn_with_state};
use axum::{Extension, Router};
use megh::auth::rest::AppContext;
use megh::auth::{AccessToken, JWT_COOKIE, JwtSession, jwt_session};
use megh::messaging::{File, Messaging, MessagingError, Notice, Party, Signal, Surface};
use megh::{Broker, Topic, authorizer};
use serde_json::{Value, json};
use sqlx::PgPool;
use tokio::sync::mpsc::{UnboundedReceiver as Receiver, unbounded_channel as channel};
use tower::ServiceExt;
use tower_sessions::{MemoryStore, SessionManagerLayer};
use uuid::Uuid;

const SECRET: &[u8] = b"identity-test-secret-of-32-bytes!";
const BASE: &str = "https://app.example.com/api/v1";

async fn tenant(pool: &PgPool, name: &str) -> Uuid {
    sqlx::query_scalar("INSERT INTO tenants (name) VALUES ($1) RETURNING id").bind(name).fetch_one(pool).await.unwrap()
}

async fn client(pool: &PgPool, tenant: Uuid, name: &str) -> Uuid {
    sqlx::query_scalar("INSERT INTO clients (tenant_id, id, first_name, last_name, name) VALUES ($1, $2, $3, $3, $3) RETURNING id")
        .bind(tenant)
        .bind(Uuid::now_v7())
        .bind(name)
        .fetch_one(pool)
        .await
        .unwrap()
}

fn jwt(pool: &PgPool) -> JwtSession {
    JwtSession::new(SECRET, Duration::from_secs(300), pool.clone())
}

fn app(pool: PgPool, broker: Broker) -> Router {
    let jwt = jwt(&pool);
    let (guests, _) = megh::messaging::guests(jwt.clone()).split_for_parts();
    let (api, _) = megh::messaging::router(1 << 20, BASE).split_for_parts();
    let api = api.layer(from_fn(authorizer)).layer(from_fn_with_state(jwt, jwt_session));
    Router::new()
        .nest("/api/v1", guests.merge(api))
        .layer(Extension(broker))
        .layer(SessionManagerLayer::new(MemoryStore::default()))
        .with_state(AppContext { pool })
}

async fn call(app: &Router, uri: &str, cookie: &str, bearer: Option<&str>, body: Value) -> Response<Body> {
    let request = Request::builder().method("POST").uri(uri).header(header::COOKIE, cookie).header(header::CONTENT_TYPE, "application/json");
    let request = bearer.into_iter().fold(request, |request, token| request.header(header::AUTHORIZATION, format!("Bearer {token}")));
    app.clone().oneshot(request.body(Body::from(body.to_string())).unwrap()).await.unwrap()
}

/// A cookie the response set, as `name=value`.
fn set(response: &Response<Body>, name: &str) -> Option<String> {
    let all = response.headers().get_all(header::SET_COOKIE).iter().map(|v| v.to_str().unwrap().to_string()).collect::<Vec<_>>();
    all.into_iter().find(|v| v.starts_with(&format!("{name}="))).map(|v| v.split(';').next().unwrap().to_string())
}

async fn json(response: Response<Body>) -> Value {
    serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap()
}

/// The browser's first visit: the guest's id, its session cookie and its token cookie.
async fn visit(app: &Router, tenant: Uuid, session: &str) -> (Uuid, String, String) {
    let response = call(app, &format!("/api/v1/tenants/{tenant}/guests"), session, None, json!({})).await;
    assert_eq!(response.status(), StatusCode::OK);
    let session = set(&response, "id").unwrap_or_else(|| session.to_string());
    let token = set(&response, JWT_COOKIE).unwrap();
    let id = json(response).await["client_id"].as_str().unwrap().parse().unwrap();
    (id, session, token)
}

async fn say(app: &Router, tenant: Uuid, member: Uuid, cookie: &str, text: &str) -> Response<Body> {
    call(app, &format!("/api/v1/tenants/{tenant}/members/{member}/messages"), cookie, None, json!({"body": {"text": text}})).await
}

async fn stored(pool: &PgPool, client: Uuid) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM messages WHERE client_id = $1").bind(client).fetch_one(pool).await.unwrap()
}

fn notices(messaging: &Messaging, me: Party) -> Receiver<Notice> {
    let (tx, rx) = channel();
    messaging.on_event(me, move |notice| tx.send(notice).unwrap());
    rx
}

#[sqlx::test(migrations = false)]
async fn a_known_client_gets_a_token_that_carries_only_the_client_routes(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let salon = tenant(&pool, "Salon").await;
    let ada = client(&pool, salon, "Ada").await;
    let app = app(pool.clone(), Broker::new());

    let issued = jwt(&pool).issue_client(salon, ada).unwrap();
    let sent = call(&app, &format!("/api/v1/tenants/{salon}/members/{}/messages", Uuid::now_v7()), "", Some(&issued.jwt), json!({"body": {"text": "hi"}})).await;

    let scope = issued.scope.iter().map(|grant| grant.as_str().to_string()).collect::<Vec<_>>();
    assert_eq!(scope, [format!("tenants:read:{salon}"), "members:read".into(), "messages:create".into(), "events:create".into(), "attachments:create".into(), "attachments:read".into()]);
    assert_eq!(sent.status(), StatusCode::OK);
    assert_eq!(stored(&pool, ada).await, 1);
}

#[sqlx::test(migrations = false)]
async fn a_known_clients_token_is_refused_in_another_tenant(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let (salon, spa) = (tenant(&pool, "Salon").await, tenant(&pool, "Spa").await);
    let ada = client(&pool, salon, "Ada").await;
    let app = app(pool.clone(), Broker::new());

    let issued = jwt(&pool).issue_client(salon, ada).unwrap();
    let sent = call(&app, &format!("/api/v1/tenants/{spa}/members/{}/messages", Uuid::now_v7()), "", Some(&issued.jwt), json!({"body": {}})).await;

    assert_eq!(sent.status(), StatusCode::FORBIDDEN);
}

#[sqlx::test(migrations = false)]
async fn a_first_visit_makes_a_guest_and_the_same_session_stays_that_guest(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let salon = tenant(&pool, "Salon").await;
    let app = app(pool.clone(), Broker::new());

    let (guest, session, token) = visit(&app, salon, "").await;
    let (again, _, _) = visit(&app, salon, &session).await;
    let names: (String, String, String) = sqlx::query_as("SELECT first_name, last_name, name FROM clients WHERE tenant_id = $1 AND id = $2 AND deleted_at IS NULL")
        .bind(salon)
        .bind(guest)
        .fetch_one(&pool)
        .await
        .unwrap();

    assert_eq!(again, guest);
    assert_eq!(names, ("Guest".into(), "Guest".into(), "Guest".into()));
    assert_eq!(say(&app, salon, Uuid::now_v7(), &token, "hello").await.status(), StatusCode::OK);
    assert_eq!(stored(&pool, guest).await, 1);
}

#[sqlx::test(migrations = false)]
async fn a_guest_in_another_tenant_is_another_guest(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let (salon, spa) = (tenant(&pool, "Salon").await, tenant(&pool, "Spa").await);
    let app = app(pool.clone(), Broker::new());

    let (at_salon, session, _) = visit(&app, salon, "").await;
    let (at_spa, _, _) = visit(&app, spa, &session).await;

    assert_ne!(at_spa, at_salon);
}

#[sqlx::test(migrations = false)]
async fn an_expired_guest_token_is_renewed_from_the_session(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let salon = tenant(&pool, "Salon").await;
    let app = app(pool.clone(), Broker::new());

    let (guest, session, _) = visit(&app, salon, "").await;
    let renewed = say(&app, salon, Uuid::now_v7(), &session, "back again").await;

    assert_eq!(renewed.status(), StatusCode::OK);
    assert!(set(&renewed, JWT_COOKIE).is_some());
    assert_eq!(stored(&pool, guest).await, 1);
}

#[sqlx::test(migrations = false)]
async fn identifying_moves_the_guests_chat_to_the_client_and_removes_the_guest(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let salon = tenant(&pool, "Salon").await;
    let ada = client(&pool, salon, "Ada").await;
    let broker = Broker::new();
    let app = app(pool.clone(), broker.clone());
    let messaging = Messaging::new(pool.clone(), broker, salon).with_files_at(BASE);
    let agent = Party::Member(Uuid::now_v7());

    let (guest, _, token) = visit(&app, salon, "").await;
    say(&app, salon, match agent { Party::Member(id) => id, Party::Client(id) => id }, &token, "book a haircut").await;
    messaging.send(Surface::new().text("Who are you?"), agent, Party::Client(guest)).await.unwrap();
    messaging.attach(File { name: "a.txt".into(), mime_type: "text/plain".into(), bytes: b"hi".to_vec() }, Party::Client(guest), agent).await.unwrap();
    let mut widget = notices(&messaging, Party::Client(guest));
    tokio::time::sleep(Duration::from_millis(100)).await;

    messaging.identify(guest, ada).await.unwrap();

    let (deleted, moved_to): (bool, Option<Uuid>) = sqlx::query_as("SELECT deleted_at IS NOT NULL, moved_to FROM clients WHERE id = $1").bind(guest).fetch_one(&pool).await.unwrap();
    let files: i64 = sqlx::query_scalar("SELECT count(*) FROM attachments WHERE client_id = $1").bind(ada).fetch_one(&pool).await.unwrap();
    let notice = tokio::time::timeout(Duration::from_secs(2), widget.recv()).await.unwrap().unwrap();
    assert_eq!((stored(&pool, guest).await, stored(&pool, ada).await, files), (0, 2, 1));
    assert_eq!((deleted, moved_to), (true, Some(ada)));
    assert_eq!((notice.from, notice.to, notice.signal), (agent, Party::Client(guest), Signal::Moved(ada)));
}

#[sqlx::test(migrations = false)]
async fn after_the_move_the_same_browser_is_the_client(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let salon = tenant(&pool, "Salon").await;
    let ada = client(&pool, salon, "Ada").await;
    let broker = Broker::new();
    let app = app(pool.clone(), broker.clone());
    let messaging = Messaging::new(pool.clone(), broker, salon);

    let (guest, session, _) = visit(&app, salon, "").await;
    messaging.identify(guest, ada).await.unwrap();
    let (now, _, token) = visit(&app, salon, &session).await;
    let renewed = say(&app, salon, Uuid::now_v7(), &session, "renewed as Ada").await;

    assert_eq!(now, ada);
    assert_eq!(say(&app, salon, Uuid::now_v7(), &token, "as Ada").await.status(), StatusCode::OK);
    assert_eq!(renewed.status(), StatusCode::OK);
    assert_eq!((stored(&pool, ada).await, stored(&pool, guest).await), (2, 0));
}

#[sqlx::test(migrations = false)]
async fn only_a_live_guest_is_identified(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let salon = tenant(&pool, "Salon").await;
    let (ada, bob) = (client(&pool, salon, "Ada").await, client(&pool, salon, "Bob").await);
    let messaging = Messaging::new(pool.clone(), Broker::new(), salon);

    messaging.identify(bob, ada).await.unwrap();

    assert!(matches!(messaging.identify(Uuid::now_v7(), ada).await, Err(MessagingError::NotFound)));
    assert!(matches!(messaging.identify(bob, ada).await, Err(MessagingError::NotFound)));
}

#[sqlx::test(migrations = false)]
async fn a_client_follows_only_its_own_topics(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let (salon, spa, ada, bob) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    let token = AccessToken { sub: ada, iss: "megh".into(), aud: vec![salon.to_string()], exp: u64::MAX, iat: 0, client_id: "megh".into(), scope: String::new() };
    let topic = |tenant: Uuid, client: Uuid| Topic::new(format!("tenants/{tenant}/clients/{client}/members/*"));

    assert!(megh::messaging::follows(Some(&token), &topic(salon, ada)));
    assert!(megh::messaging::follows(Some(&token), &Topic::new(format!("tenants/{salon}/clients/{ada}/members/{}", Uuid::now_v7()))));
    assert!(!megh::messaging::follows(Some(&token), &topic(salon, bob)));
    assert!(!megh::messaging::follows(Some(&token), &topic(spa, ada)));
    assert!(!megh::messaging::follows(None, &topic(salon, ada)));
}

#[sqlx::test(migrations = false)]
async fn a_member_follows_only_the_topics_it_is_on(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let (salon, spa, mia, sam, ada) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    let token = AccessToken { sub: mia, iss: "megh".into(), aud: vec![salon.to_string()], exp: u64::MAX, iat: 0, client_id: "megh".into(), scope: String::new() };
    let topic = |tenant: Uuid, client: &str, member: Uuid| Topic::new(format!("tenants/{tenant}/clients/{client}/members/{member}"));

    assert!(megh::messaging::follows(Some(&token), &topic(salon, "*", mia)));
    assert!(megh::messaging::follows(Some(&token), &topic(salon, &ada.to_string(), mia)));
    assert!(!megh::messaging::follows(Some(&token), &topic(salon, "*", sam)));
    assert!(!megh::messaging::follows(Some(&token), &topic(spa, "*", mia)));
}

#[sqlx::test(migrations = false)]
async fn both_sides_hear_each_other_on_a_broker_that_lets_each_follow_only_its_own(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let salon = tenant(&pool, "Salon").await;
    let messaging = Messaging::new(pool.clone(), Broker::new().with_authorizer(megh::messaging::follows), salon);
    let (agent, ada) = (Party::Member(Uuid::now_v7()), Party::Client(Uuid::now_v7()));
    let (heard, mut agent_heard) = channel();
    messaging.on(agent, move |message| heard.send(message).unwrap());
    let (told, mut ada_heard) = channel();
    messaging.on(ada, move |message| told.send(message).unwrap());
    tokio::time::sleep(Duration::from_millis(100)).await;

    messaging.send(Surface::new().text("hi"), ada, agent).await.unwrap();
    messaging.send(Surface::new().text("hello"), agent, ada).await.unwrap();

    let within = Duration::from_secs(2);
    assert_eq!(tokio::time::timeout(within, agent_heard.recv()).await.unwrap().unwrap().from, ada);
    assert_eq!(tokio::time::timeout(within, ada_heard.recv()).await.unwrap().unwrap().from, agent);
}
