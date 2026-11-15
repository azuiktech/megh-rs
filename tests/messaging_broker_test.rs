//! Messaging over the broker, between a client and a member of one tenant: `send(card, from, to)`,
//! `send_event(signal, from, to)`, `on` and `on_event`, the one-time display configuration, and the browser's routes.

use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::middleware::from_fn;
use axum::{Extension, Router};
use megh::auth::AccessToken;
use megh::auth::rest::AppContext;
use megh::messaging::{Avatar, History, Icon, Message, Messaging, Notice, Party, ReadMark, Signal, Surface, TimeFormat};
use megh::{Broker, Grant, HistoryOptions, HistoryProvider, Topic, authorizer};
use serde_json::{Value, json};
use sqlx::PgPool;
use tokio::sync::mpsc::{UnboundedReceiver as Receiver, unbounded_channel as channel};
use tower::ServiceExt;
use uuid::Uuid;

async fn tenant(pool: &PgPool, name: &str) -> Uuid {
    sqlx::query_scalar("INSERT INTO tenants (name) VALUES ($1) RETURNING id").bind(name).fetch_one(pool).await.unwrap()
}

fn card(text: &str) -> Surface {
    Surface::new().text(text).row(|r| r.button("yes", "approve").button("no", "reject"))
}

fn messages(messaging: &Messaging, me: Party) -> Receiver<Message> {
    let (tx, rx) = channel();
    messaging.on(me, move |message| tx.send(message).unwrap());
    rx
}

fn notices(messaging: &Messaging, me: Party) -> Receiver<Notice> {
    let (tx, rx) = channel();
    messaging.on_event(me, move |notice| tx.send(notice).unwrap());
    rx
}

async fn next<T>(rx: &mut Receiver<T>) -> T {
    tokio::time::timeout(Duration::from_secs(2), rx.recv()).await.unwrap().unwrap()
}

async fn nothing<T: std::fmt::Debug>(rx: &mut Receiver<T>) {
    assert!(tokio::time::timeout(Duration::from_millis(300), rx.recv()).await.is_err());
}

fn components(message: &Message) -> Vec<Value> {
    message.body[1]["updateComponents"]["components"].as_array().unwrap().clone()
}

async fn settle() {
    tokio::time::sleep(Duration::from_millis(100)).await;
}

#[sqlx::test(migrations = false)]
async fn a_member_sends_a_card_to_a_client_and_the_client_hears_it(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let messaging = Messaging::new(pool.clone(), Broker::new(), tenant(&pool, "Salon").await);
    let (agent, ann) = (Party::Member(Uuid::now_v7()), Party::Client(Uuid::now_v7()));
    let mut heard = messages(&messaging, ann);
    settle().await;

    let sent = messaging.send(card("Do you want to book this appointment?"), agent, ann).await.unwrap();

    let got = next(&mut heard).await;
    assert_eq!(got, sent);
    assert_eq!((got.from, got.to), (agent, ann));
    assert_eq!(got.body[0]["version"], "1.0");
    assert!(components(&got).iter().any(|c| c["text"] == "Do you want to book this appointment?"));
}

#[sqlx::test(migrations = false)]
async fn the_member_hears_what_its_clients_send_but_not_its_own(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let messaging = Messaging::new(pool.clone(), Broker::new(), tenant(&pool, "Salon").await);
    let (agent, ann, bob) = (Party::Member(Uuid::now_v7()), Party::Client(Uuid::now_v7()), Party::Client(Uuid::now_v7()));
    let mut heard = messages(&messaging, agent);
    settle().await;

    messaging.send(card("hi"), agent, ann).await.unwrap();
    let from_ann = messaging.send(Surface::new().text("from ann"), ann, agent).await.unwrap();
    let from_bob = messaging.send(Surface::new().text("from bob"), bob, agent).await.unwrap();

    assert_eq!(next(&mut heard).await.id, from_ann.id);
    assert_eq!(next(&mut heard).await.id, from_bob.id);
    nothing(&mut heard).await;
}

#[sqlx::test(migrations = false)]
async fn messages_are_only_between_a_client_and_a_member(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let messaging = Messaging::new(pool.clone(), Broker::new(), tenant(&pool, "Salon").await);
    let (a, b) = (Party::Member(Uuid::now_v7()), Party::Member(Uuid::now_v7()));
    assert!(messaging.send(card("x"), a, b).await.is_err());
    assert!(messaging.send(card("x"), Party::Client(Uuid::now_v7()), Party::Client(Uuid::now_v7())).await.is_err());
}

#[sqlx::test(migrations = false)]
async fn one_tenants_messages_do_not_reach_another(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let broker = Broker::new();
    let salon = Messaging::new(pool.clone(), broker.clone(), tenant(&pool, "Salon").await);
    let spa = Messaging::new(pool.clone(), broker, tenant(&pool, "Spa").await);
    let (agent, ann) = (Party::Member(Uuid::now_v7()), Party::Client(Uuid::now_v7()));
    let mut in_spa = messages(&spa, ann);
    settle().await;

    salon.send(card("hi"), agent, ann).await.unwrap();

    nothing(&mut in_spa).await;
}

#[sqlx::test(migrations = false)]
async fn typing_is_an_event_the_other_side_hears(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let messaging = Messaging::new(pool.clone(), Broker::new(), tenant(&pool, "Salon").await);
    let (agent, ann) = (Party::Member(Uuid::now_v7()), Party::Client(Uuid::now_v7()));
    let mut at_ann = notices(&messaging, ann);
    let mut at_agent = notices(&messaging, agent);
    settle().await;

    messaging.send_event(Signal::TypingOn, agent, ann).await.unwrap();
    messaging.send_event(Signal::TypingOff, agent, ann).await.unwrap();

    assert_eq!(next(&mut at_ann).await, Notice { from: agent, to: ann, signal: Signal::TypingOn, updates: vec![] });
    assert_eq!(next(&mut at_ann).await.signal, Signal::TypingOff);
    nothing(&mut at_agent).await;
}

#[sqlx::test(migrations = false)]
async fn avatar_timestamp_and_read_mark_are_set_once_and_frame_every_message(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let messaging = Messaging::new(pool.clone(), Broker::new(), tenant(&pool, "Salon").await)
        .with_avatar(|sender| Avatar::Image(format!("https://img.example.com/{sender}.png")))
        .with_timestamp(TimeFormat("%H:%M"))
        .with_read_mark(ReadMark { sent: Icon::name("check"), read: Icon::name("doneAll") });
    let (agent_id, ann) = (Uuid::now_v7(), Party::Client(Uuid::now_v7()));

    let sent = messaging.send(card("Book?"), Party::Member(agent_id), ann).await.unwrap();

    let all = components(&sent);
    let image = all.iter().find(|c| c["component"] == "Image").unwrap();
    assert_eq!((image["variant"].clone(), image["url"].clone()), (json!("avatar"), json!(format!("https://img.example.com/{agent_id}.png"))));
    let time = all.iter().find(|c| c["variant"] == "caption").unwrap()["text"].as_str().unwrap().to_string();
    assert!(time.len() == 5 && time.as_bytes()[2] == b':');
    assert_eq!(all.iter().find(|c| c["component"] == "Icon").unwrap()["name"], json!({"@path": "/meta/mark"}));
    assert_eq!(sent.body[0]["createSurface"]["dataModel"]["meta"]["mark"], "check");
}

#[sqlx::test(migrations = false)]
async fn without_configuration_a_message_is_the_surface_alone(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let messaging = Messaging::new(pool.clone(), Broker::new(), tenant(&pool, "Salon").await);
    let sent = messaging.send(Surface::new().text("Hi"), Party::Member(Uuid::now_v7()), Party::Client(Uuid::now_v7())).await.unwrap();
    let all = components(&sent);
    assert_eq!(all.iter().map(|c| c["component"].clone()).collect::<Vec<_>>(), [json!("Column"), json!("Text")]);
}

#[sqlx::test(migrations = false)]
async fn reading_turns_the_mark_to_read_for_the_sender_and_in_history(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let messaging = Messaging::new(pool.clone(), Broker::new(), tenant(&pool, "Salon").await)
        .with_read_mark(ReadMark { sent: Icon::name("check"), read: Icon::name("doneAll") });
    let (agent, ann) = (Party::Member(Uuid::now_v7()), Party::Client(Uuid::now_v7()));
    let mut at_agent = notices(&messaging, agent);
    settle().await;
    let sent = messaging.send(card("Book?"), agent, ann).await.unwrap();

    messaging.send_event(Signal::Read(sent.id), ann, agent).await.unwrap();

    let notice = next(&mut at_agent).await;
    assert_eq!(notice.signal, Signal::Read(sent.id));
    let surface_id = sent.body[0]["createSurface"]["surfaceId"].clone();
    assert_eq!(notice.updates, vec![json!({"version": "1.0", "updateDataModel": {"surfaceId": surface_id, "path": "/meta/mark", "value": "doneAll"}})]);
    let stored = sqlx::query_scalar::<_, Value>("SELECT body FROM messages WHERE id = $1").bind(sent.id).fetch_one(&pool).await.unwrap();
    assert_eq!(stored[0]["createSurface"]["dataModel"]["meta"]["mark"], "doneAll");
}

#[sqlx::test(migrations = false)]
async fn a_reconnecting_browser_catches_up_from_the_database(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let salon = tenant(&pool, "Salon").await;
    let messaging = Messaging::new(pool.clone(), Broker::new(), salon);
    let (agent_id, ann_id) = (Uuid::now_v7(), Uuid::now_v7());
    let (agent, ann) = (Party::Member(agent_id), Party::Client(ann_id));
    let first = messaging.send(card("one"), agent, ann).await.unwrap();
    let second = messaging.send(card("two"), agent, ann).await.unwrap();
    messaging.send(card("to bob"), agent, Party::Client(Uuid::now_v7())).await.unwrap();
    let history = History::new(pool.clone());
    let pair = Topic::new(format!("tenants/{salon}/clients/{ann_id}/members/{agent_id}"));

    let missed = history.fetch_history_since_id(&pair, &first.id.to_string(), None).await.unwrap();

    assert_eq!(missed.iter().map(|e| e.id.clone()).collect::<Vec<_>>(), [second.id.to_string()]);
    assert_eq!(missed[0].topic, pair);
    assert_eq!(missed[0].data["kind"], "message");
    let all = history.fetch_history(&pair, &HistoryOptions { since: None, limit: Some(50) }).await.unwrap();
    assert_eq!(all.len(), 2);
    let everyone = Topic::new(format!("tenants/{salon}/clients/*/members/{agent_id}"));
    assert_eq!(history.fetch_history(&everyone, &HistoryOptions { since: None, limit: None }).await.unwrap().len(), 3);
}

fn app(pool: PgPool, broker: Broker, tenant: Uuid, client: Uuid) -> Router {
    let (api, _) = megh::messaging::router().split_for_parts();
    let grants = [format!("tenants:read:{tenant}"), "members:*".into(), "messages:*".into(), "events:*".into()].map(Grant::new).to_vec();
    let token = AccessToken { sub: client, iss: "megh".into(), aud: vec![tenant.to_string()], exp: u64::MAX, iat: 0, client_id: "megh".into(), scope: String::new() };
    Router::new()
        .nest("/api/v1", api.layer(from_fn(authorizer)))
        .layer(Extension(grants))
        .layer(Extension(token))
        .layer(Extension(broker))
        .with_state(AppContext { pool })
}

async fn post(app: &Router, uri: &str, body: Value) -> StatusCode {
    let request = Request::builder().method("POST").uri(uri).header("content-type", "application/json");
    app.clone().oneshot(request.body(Body::from(body.to_string())).unwrap()).await.unwrap().status()
}

#[sqlx::test(migrations = false)]
async fn the_browser_sends_answers_and_events_to_its_member(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let (broker, salon, agent_id, ann_id) = (Broker::new(), tenant(&pool, "Salon").await, Uuid::now_v7(), Uuid::now_v7());
    let messaging = Messaging::new(pool.clone(), broker.clone(), salon);
    let (mut heard, mut noticed) = (messages(&messaging, Party::Member(agent_id)), notices(&messaging, Party::Member(agent_id)));
    settle().await;
    let app = app(pool.clone(), broker, salon, ann_id);
    let answer = json!({"name": "approve", "surfaceId": "s1", "sourceComponentId": "c1", "timestamp": "2026-11-15T10:00:00Z", "context": {}});

    let sent = post(&app, &format!("/api/v1/tenants/{salon}/members/{agent_id}/messages"), json!({"body": answer})).await;
    let signalled = post(&app, &format!("/api/v1/tenants/{salon}/members/{agent_id}/events"), json!({"signal": "typing_on"})).await;

    assert_eq!((sent, signalled), (StatusCode::OK, StatusCode::NO_CONTENT));
    let message = next(&mut heard).await;
    assert_eq!((message.from, message.body.clone()), (Party::Client(ann_id), answer));
    assert_eq!(next(&mut noticed).await.signal, Signal::TypingOn);
}
