//! Messaging a2ui surfaces (docs/TPD-messaging.md): validation against a2ui v1.0, the ready-made builders and their
//! Messenger limits, sending a surface, answering it with `:respond`, and the typed answers.

use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::middleware::from_fn;
use axum::{Extension, Router};
use megh::auth::AccessToken;
use megh::auth::rest::{AppContext, PageRequest};
use megh::billing::invoice::Invoice;
use megh::billing::payment::Payment;
use megh::booking::availability::Slot;
use megh::booking::value::Name;
use megh::messaging::surface::{
    Button, Card, Choice, Dates, Decision, Intake, Prompt, Rated, Reply, Scale, Selected, Surface,
    answer,
};
use megh::messaging::{Content, Messaging, MessagingError, Party};
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

fn name(text: &str) -> Name {
    Name::try_new(text).unwrap()
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

fn json_of<T: serde::Serialize>(value: &T) -> Value {
    serde_json::to_value(value).unwrap()
}

fn prompt() -> Prompt {
    Prompt::new("Pick one")
}

fn choices(count: usize) -> Vec<Choice> {
    (0..count)
        .map(|n| Choice::new(Uuid::now_v7(), format!("Choice {n}")))
        .collect()
}

fn slots() -> Vec<Slot> {
    let slot = json!({"parties": [{
        "client": {"id": Uuid::now_v7()}, "service_id": Uuid::now_v7(),
        "providers": [{"id": Uuid::now_v7()}], "resources": [{"id": Uuid::now_v7()}],
        "starts_at": "2026-11-20T09:00:00+00:00", "ends_at": "2026-11-20T09:45:00+00:00",
        "waitlist": false, "capacity": 1
    }]});
    vec![serde_json::from_value(slot).unwrap()]
}

fn replies(count: usize) -> Vec<Reply> {
    (0..count)
        .map(|n| Reply::new(format!("Reply {n}"), format!("reply-{n}")))
        .collect()
}

fn buttons(count: usize) -> Vec<Button> {
    (0..count)
        .map(|n| Button::new(format!("Button {n}"), format!("button-{n}")))
        .collect()
}

fn cards(count: usize) -> Vec<Card> {
    (0..count).map(|n| Card::new(format!("Card {n}"))).collect()
}

async fn billed(pool: &PgPool, tenant: Uuid) -> (Invoice, Payment) {
    let invoice_id: Uuid = sqlx::query_scalar(
        "INSERT INTO billing_invoices (tenant_id, id, party_id, party_kind, kind, number, status, date, due_date, currency, total, balance_due)
         VALUES ($1, uuidv7(), $1, 'tenant', 'receivable', 'INV-1', 'open', '2026-11-15', '2026-12-15', 'USD', 100000000, 0) RETURNING id",
    )
    .bind(tenant)
    .fetch_one(pool)
    .await
    .unwrap();
    let payment_id: Uuid = sqlx::query_scalar(
        "INSERT INTO billing_payments (tenant_id, id, invoice_id, date, amount, method, status)
         VALUES ($1, uuidv7(), $2, '2026-11-15', 100000000, 'card', 'succeeded') RETURNING id",
    )
    .bind(tenant)
    .bind(invoice_id)
    .fetch_one(pool)
    .await
    .unwrap();
    let invoice = sqlx::query_as("SELECT * FROM billing_invoices WHERE tenant_id = $1 AND id = $2")
        .bind(tenant)
        .bind(invoice_id)
        .fetch_one(pool)
        .await
        .unwrap();
    let payment = sqlx::query_as("SELECT * FROM billing_payments WHERE tenant_id = $1 AND id = $2")
        .bind(tenant)
        .bind(payment_id)
        .fetch_one(pool)
        .await
        .unwrap();
    (invoice, payment)
}

fn debug<T: std::fmt::Debug>(value: &T) -> String {
    format!("{value:?}")
}

#[test]
fn generic_accepts_the_messages_of_a_valid_a2ui_surface() {
    let valid = Surface::selection(prompt(), choices(2));

    let surface = Surface::generic(valid.messages.clone());

    assert!(surface.is_ok(), "{:?}", surface.err());
}

#[test]
fn generic_rejects_a_message_that_is_not_a2ui() {
    let unknown_type = Surface::generic(vec![json!({"version": "v1.0", "explode": {}})]);
    let not_an_object = Surface::generic(vec![json!("hello")]);

    assert!(matches!(
        unknown_type,
        Err(MessagingError::InvalidSurface(_))
    ));
    assert!(matches!(
        not_an_object,
        Err(MessagingError::InvalidSurface(_))
    ));
}

#[test]
fn generic_rejects_a_message_of_another_a2ui_version() {
    let mut messages = Surface::selection(prompt(), choices(2)).messages;
    messages
        .iter_mut()
        .for_each(|message| message["version"] = json!("v0.9"));

    let surface = Surface::generic(messages);

    assert!(matches!(surface, Err(MessagingError::InvalidSurface(_))));
}

#[sqlx::test(migrations = false)]
async fn every_ready_made_surface_validates_against_a2ui_v1(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let acme = tenant(&pool, "Acme").await;
    let (invoice, payment) = billed(&pool, acme).await;
    let surfaces = vec![
        ("selection", Surface::selection(prompt(), choices(3))),
        ("quantities", Surface::quantities(prompt(), choices(3))),
        ("login", Surface::login(prompt())),
        ("logout", Surface::logout(prompt())),
        ("open_times", Surface::open_times(prompt(), slots())),
        ("calendar_date", Surface::calendar_date(prompt())),
        ("calendar_range", Surface::calendar_range(prompt())),
        ("intake", Surface::intake(prompt())),
        (
            "feedback_rating",
            Surface::feedback_rating(prompt(), Scale::new(5)),
        ),
        (
            "feedback_choice",
            Surface::feedback_choice(prompt(), vec![name("Good"), name("Bad")]),
        ),
        (
            "approval",
            Surface::approval(prompt(), vec![(name("Service"), "Haircut".to_string())]),
        ),
        ("quick_replies", Surface::quick_replies(replies(3)).unwrap()),
        ("buttons", Surface::buttons("Choose", buttons(3)).unwrap()),
        ("carousel", Surface::carousel(cards(3)).unwrap()),
        ("receipt", Surface::receipt(&invoice, &payment).unwrap()),
    ];

    surfaces.into_iter().for_each(|(builder, surface)| {
        let validated = Surface::generic(surface.messages.clone());
        assert!(validated.is_ok(), "{builder}: {:?}", validated.err());
    });
}

#[test]
fn thirteen_quick_replies_are_accepted_and_fourteen_are_an_error() {
    assert!(Surface::quick_replies(replies(13)).is_ok());
    assert!(Surface::quick_replies(replies(14)).is_err());
}

#[test]
fn a_quick_reply_title_over_twenty_characters_is_an_error() {
    let long = Reply::new("x".repeat(21), "payload");
    let fits = Reply::new("x".repeat(20), "payload");

    assert!(Surface::quick_replies(vec![fits]).is_ok());
    assert!(Surface::quick_replies(vec![long]).is_err());
}

#[test]
fn three_buttons_are_accepted_and_four_are_an_error() {
    assert!(Surface::buttons("Choose", buttons(3)).is_ok());
    assert!(Surface::buttons("Choose", buttons(4)).is_err());
}

#[test]
fn ten_carousel_cards_are_accepted_and_eleven_are_an_error() {
    assert!(Surface::carousel(cards(10)).is_ok());
    assert!(Surface::carousel(cards(11)).is_err());
}

#[sqlx::test(migrations = false)]
async fn a_sent_surface_is_stored_as_a_surface_message_with_its_surface_id(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let (messaging, _) = messaging(&pool);
    let acme = tenant(&pool, "Acme").await;
    let ann = client(&pool, acme, "Ann").await;
    let bot = agent(&pool, acme, "assistant").await;
    let thread = messaging
        .open(acme, Party::Client(ann), None)
        .await
        .unwrap();
    let speaker = messaging
        .join(acme, thread.id, Party::Member(bot))
        .await
        .unwrap();
    let surface = Surface::selection(prompt(), choices(2));

    let sent = messaging
        .send(
            acme,
            thread.id,
            speaker.id,
            Content::surface(surface.clone()),
        )
        .await
        .unwrap();

    let stored: (String, Option<String>, Option<Value>) = sqlx::query_as(
        "SELECT kind, surface_id, surface FROM messages WHERE tenant_id = $1 AND id = $2",
    )
    .bind(acme)
    .bind(sent.id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        stored,
        (
            "surface".into(),
            Some(surface.id.clone()),
            Some(json!(surface.messages))
        )
    );
}

#[sqlx::test(migrations = false)]
async fn posting_a2ui_messages_over_http_sends_a_surface_message(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let (messaging, _) = messaging(&pool);
    let acme = tenant(&pool, "Acme").await;
    let ann = client(&pool, acme, "Ann").await;
    let bot = agent(&pool, acme, "assistant").await;
    let thread = messaging
        .open(acme, Party::Client(ann), None)
        .await
        .unwrap();
    let app = full_app(pool, messaging, acme, bot);
    let surface = Surface::selection(prompt(), choices(2));

    let (status, message) = call(
        &app,
        "POST",
        &format!("/api/v1/tenants/{acme}/threads/{}/messages", thread.id),
        json!({"surface": surface.messages}),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "{message}");
    assert_eq!(
        (message["kind"].clone(), message["surface_id"].clone()),
        (json!("surface"), json!(surface.id))
    );
}

#[sqlx::test(migrations = false)]
async fn posting_an_invalid_surface_over_http_is_a_client_error_and_stores_nothing(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let (messaging, _) = messaging(&pool);
    let acme = tenant(&pool, "Acme").await;
    let ann = client(&pool, acme, "Ann").await;
    let bot = agent(&pool, acme, "assistant").await;
    let thread = messaging
        .open(acme, Party::Client(ann), None)
        .await
        .unwrap();
    let app = full_app(pool.clone(), messaging, acme, bot);

    let (status, _) = call(
        &app,
        "POST",
        &format!("/api/v1/tenants/{acme}/threads/{}/messages", thread.id),
        json!({"surface": [{"version": "v1.0", "explode": {}}]}),
    )
    .await;

    let messages: i64 = sqlx::query_scalar("SELECT count(*) FROM messages")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(status.is_client_error(), "{status}");
    assert_eq!(messages, 0);
}

struct Asked {
    pool: PgPool,
    messaging: Messaging,
    tenant: Uuid,
    thread: Uuid,
    ann: Uuid,
    surface_message: Uuid,
    surface_id: String,
}

async fn asked(pool: &PgPool, messaging: Messaging, surface: Surface) -> Asked {
    let acme = tenant(pool, "Acme").await;
    let ann = client(pool, acme, "Ann").await;
    let bot = agent(pool, acme, "assistant").await;
    let thread = messaging
        .open(acme, Party::Client(ann), None)
        .await
        .unwrap();
    let speaker = messaging
        .join(acme, thread.id, Party::Member(bot))
        .await
        .unwrap();
    let sent = messaging
        .send(
            acme,
            thread.id,
            speaker.id,
            Content::surface(surface.clone()),
        )
        .await
        .unwrap();
    Asked {
        pool: pool.clone(),
        messaging,
        tenant: acme,
        thread: thread.id,
        ann,
        surface_message: sent.id,
        surface_id: surface.id,
    }
}

async fn respond(asked: &Asked, action: Value) -> (StatusCode, Value) {
    let app = full_app(
        asked.pool.clone(),
        asked.messaging.clone(),
        asked.tenant,
        asked.ann,
    );
    let uri = format!(
        "/api/v1/tenants/{}/threads/{}/messages/{}:respond",
        asked.tenant, asked.thread, asked.surface_message
    );
    call(&app, "POST", &uri, action).await
}

async fn answer_to(asked: &Asked, context: Value) -> megh::messaging::Message {
    let (status, body) = respond(asked, json!({"name": "submit", "context": context})).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let page = asked
        .messaging
        .messages(
            asked.tenant,
            asked.thread,
            PageRequest {
                page_size: Some(100),
                page_token: None,
            },
        )
        .await
        .unwrap();
    page.items
        .into_iter()
        .find(|message| json_of(message)["kind"] == json!("answer"))
        .expect("an answer message")
}

#[sqlx::test(migrations = false)]
async fn responding_stores_an_answer_message_for_the_surface(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let (messaging, _) = messaging(&pool);
    let surface = Surface::selection(prompt(), choices(2));
    let asked = asked(&pool, messaging, surface).await;
    let context = json!({"selected": [{"id": Uuid::now_v7()}]});

    let (status, stored) = respond(
        &asked,
        json!({"name": "submit", "context": context, "userMessage": "That one"}),
    )
    .await;

    assert_eq!(status, StatusCode::OK, "{stored}");
    assert_eq!(
        (
            stored["kind"].clone(),
            stored["surface_id"].clone(),
            stored["in_reply_to"].clone(),
            stored["text"].clone()
        ),
        (
            json!("answer"),
            json!(asked.surface_id),
            json!(asked.surface_message),
            json!("That one")
        )
    );
    assert_eq!(
        stored["answer"],
        json!({"name": "submit", "context": context, "userMessage": "That one"})
    );
}

#[sqlx::test(migrations = false)]
async fn the_answer_is_sent_by_the_responder(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let (messaging, _) = messaging(&pool);
    let asked = asked(&pool, messaging.clone(), Surface::calendar_date(prompt())).await;

    let (_, stored) = respond(
        &asked,
        json!({"name": "submit", "context": {"from": "2026-11-20"}}),
    )
    .await;

    let ann_speaks = messaging
        .speaker(asked.tenant, asked.thread, asked.ann)
        .await
        .unwrap();
    assert_eq!(stored["sender_id"], json!(ann_speaks.id));
}

#[sqlx::test(migrations = false)]
async fn the_answer_is_published_on_the_thread_topic(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let (messaging, broker) = messaging(&pool);
    let asked = asked(&pool, messaging, Surface::calendar_date(prompt())).await;
    let mut events = watch(
        &broker,
        &[format!("tenants/{}/threads/{}", asked.tenant, asked.thread)],
    )
    .await;

    let (_, stored) = respond(
        &asked,
        json!({"name": "submit", "context": {"from": "2026-11-20"}}),
    )
    .await;

    let event = next(&mut events).await;
    assert_eq!(
        (
            event.data["kind"].clone(),
            event.data["message"]["id"].clone(),
            event.data["message"]["kind"].clone()
        ),
        (json!("message"), stored["id"].clone(), json!("answer"))
    );
}

#[sqlx::test(migrations = false)]
async fn responding_to_a_message_that_is_not_a_surface_is_a_client_error(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let (messaging, _) = messaging(&pool);
    let mut asked = asked(&pool, messaging.clone(), Surface::calendar_date(prompt())).await;
    let speaker = messaging
        .speaker(asked.tenant, asked.thread, asked.ann)
        .await
        .unwrap();
    asked.surface_message = messaging
        .send(
            asked.tenant,
            asked.thread,
            speaker.id,
            Content::text("just text"),
        )
        .await
        .unwrap()
        .id;

    let (status, _) = respond(&asked, json!({"name": "submit", "context": {}})).await;

    assert!(status.is_client_error(), "{status}");
}

#[sqlx::test(migrations = false)]
async fn a_selection_answer_reads_as_the_selected_ids(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let (messaging, _) = messaging(&pool);
    let asked = asked(&pool, messaging, Surface::selection(prompt(), choices(2))).await;
    let (one, two) = (Uuid::now_v7(), Uuid::now_v7());

    let message = answer_to(&asked, json!({"selected": [{"id": one}, {"id": two}]})).await;

    let selected = answer::<Selected>(&message).unwrap();
    assert!(
        debug(&selected).contains(&one.to_string()) && debug(&selected).contains(&two.to_string()),
        "{selected:?}"
    );
}

#[sqlx::test(migrations = false)]
async fn a_quantities_answer_reads_as_ids_with_their_counts(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let (messaging, _) = messaging(&pool);
    let asked = asked(&pool, messaging, Surface::quantities(prompt(), choices(2))).await;
    let one = Uuid::now_v7();

    let message = answer_to(&asked, json!({"selected": [{"id": one, "count": 3}]})).await;

    let selected = answer::<Selected>(&message).unwrap();
    assert!(
        debug(&selected).contains(&one.to_string()) && debug(&selected).contains('3'),
        "{selected:?}"
    );
}

#[sqlx::test(migrations = false)]
async fn a_calendar_date_answer_reads_as_its_date(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let (messaging, _) = messaging(&pool);
    let asked = asked(&pool, messaging, Surface::calendar_date(prompt())).await;

    let message = answer_to(&asked, json!({"from": "2026-11-20"})).await;

    let dates = answer::<Dates>(&message).unwrap();
    assert!(debug(&dates).contains("2026-11-20"), "{dates:?}");
}

#[sqlx::test(migrations = false)]
async fn a_calendar_range_answer_reads_as_both_dates(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let (messaging, _) = messaging(&pool);
    let asked = asked(&pool, messaging, Surface::calendar_range(prompt())).await;

    let message = answer_to(&asked, json!({"from": "2026-11-20", "to": "2026-11-27"})).await;

    let dates = answer::<Dates>(&message).unwrap();
    assert!(
        debug(&dates).contains("2026-11-20") && debug(&dates).contains("2026-11-27"),
        "{dates:?}"
    );
}

#[sqlx::test(migrations = false)]
async fn an_intake_answer_reads_as_the_fields_of_a_client(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let (messaging, _) = messaging(&pool);
    let asked = asked(&pool, messaging, Surface::intake(prompt())).await;

    let message = answer_to(&asked, json!({"first_name": "Ada", "last_name": "Lovelace", "name": "Ada", "email": "ada@example.com"})).await;

    let intake = answer::<Intake>(&message).unwrap();
    assert!(
        debug(&intake).contains("Lovelace") && debug(&intake).contains("ada@example.com"),
        "{intake:?}"
    );
}

#[sqlx::test(migrations = false)]
async fn a_rating_answer_reads_as_its_rating_and_comment(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let (messaging, _) = messaging(&pool);
    let asked = asked(
        &pool,
        messaging,
        Surface::feedback_rating(prompt(), Scale::new(5)),
    )
    .await;

    let message = answer_to(&asked, json!({"rating": 4, "comment": "Lovely cut"})).await;

    let rated = answer::<Rated>(&message).unwrap();
    assert!(
        debug(&rated).contains('4') && debug(&rated).contains("Lovely cut"),
        "{rated:?}"
    );
}

#[sqlx::test(migrations = false)]
async fn a_choice_feedback_answer_reads_as_its_choice(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let (messaging, _) = messaging(&pool);
    let asked = asked(
        &pool,
        messaging,
        Surface::feedback_choice(prompt(), vec![name("Good"), name("Bad")]),
    )
    .await;

    let message = answer_to(&asked, json!({"choice": "Good"})).await;

    let rated = answer::<Rated>(&message).unwrap();
    assert!(debug(&rated).contains("Good"), "{rated:?}");
}

#[sqlx::test(migrations = false)]
async fn an_approval_answer_reads_as_the_decision_taken(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let (messaging, _) = messaging(&pool);
    let approving = asked(
        &pool,
        messaging.clone(),
        Surface::approval(prompt(), vec![(name("Service"), "Haircut".into())]),
    )
    .await;
    let rejecting = asked(
        &pool,
        messaging,
        Surface::approval(prompt(), vec![(name("Service"), "Haircut".into())]),
    )
    .await;

    let approved =
        answer::<Decision>(&answer_to(&approving, json!({"decision": "approved"})).await).unwrap();
    let rejected =
        answer::<Decision>(&answer_to(&rejecting, json!({"decision": "rejected"})).await).unwrap();

    assert!(
        debug(&approved).to_lowercase().contains("approved"),
        "{approved:?}"
    );
    assert!(
        debug(&rejected).to_lowercase().contains("rejected"),
        "{rejected:?}"
    );
}

#[sqlx::test(migrations = false)]
async fn an_answer_with_the_context_of_another_surface_is_refused(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let (messaging, _) = messaging(&pool);
    let dated = asked(&pool, messaging.clone(), Surface::calendar_date(prompt())).await;
    let chosen = asked(
        &pool,
        messaging.clone(),
        Surface::selection(prompt(), choices(2)),
    )
    .await;
    let approving = asked(
        &pool,
        messaging,
        Surface::approval(prompt(), vec![(name("Service"), "Haircut".into())]),
    )
    .await;

    let date_as_selection =
        answer::<Selected>(&answer_to(&dated, json!({"from": "2026-11-20"})).await);
    let selection_as_dates =
        answer::<Dates>(&answer_to(&chosen, json!({"selected": [{"id": Uuid::now_v7()}]})).await);
    let date_as_decision =
        answer::<Decision>(&answer_to(&approving, json!({"from": "2026-11-20"})).await);

    assert!(date_as_selection.is_err() && selection_as_dates.is_err() && date_as_decision.is_err());
}

#[sqlx::test(migrations = false)]
async fn an_approval_decision_other_than_approved_or_rejected_is_refused(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let (messaging, _) = messaging(&pool);
    let asked = asked(
        &pool,
        messaging,
        Surface::approval(prompt(), vec![(name("Service"), "Haircut".into())]),
    )
    .await;

    let message = answer_to(&asked, json!({"decision": "maybe"})).await;

    assert!(answer::<Decision>(&message).is_err());
}
