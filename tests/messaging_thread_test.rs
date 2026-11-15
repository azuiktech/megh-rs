//! Messaging core (docs/TPD-messaging.md): threads, participants, messages, tenant isolation, the grants of the HTTP
//! routes, speakers, and the events published on the broker.

use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::middleware::from_fn;
use axum::{Extension, Router};
use megh::auth::AccessToken;
use megh::auth::rest::{AppContext, PageRequest};
use megh::booking::value::Name;
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

#[sqlx::test(migrations = false)]
async fn opening_a_thread_for_a_client_makes_the_client_its_participant(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let acme = tenant(&pool, "Acme").await;
    let ann = client(&pool, acme, "Ann").await;
    let (messaging, _) = messaging(&pool);

    let thread = messaging
        .open(acme, Party::Client(ann), Some(name("Haircut question")))
        .await
        .unwrap();

    assert_eq!(json_of(&thread)["subject"], json!("Haircut question"));
    let participants: Vec<(Option<Uuid>, Option<Uuid>)> = sqlx::query_as(
        "SELECT member_id, client_id FROM participants WHERE tenant_id = $1 AND thread_id = $2",
    )
    .bind(acme)
    .bind(thread.id)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(participants, [(None, Some(ann))]);
}

#[sqlx::test(migrations = false)]
async fn a_machine_member_joins_a_thread_as_a_member_participant(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let acme = tenant(&pool, "Acme").await;
    let ann = client(&pool, acme, "Ann").await;
    let bot = agent(&pool, acme, "assistant").await;
    let (messaging, _) = messaging(&pool);
    let thread = messaging
        .open(acme, Party::Client(ann), None)
        .await
        .unwrap();

    let joined = messaging
        .join(acme, thread.id, Party::Member(bot))
        .await
        .unwrap();

    let (member_id, client_id): (Option<Uuid>, Option<Uuid>) = sqlx::query_as(
        "SELECT member_id, client_id FROM participants WHERE tenant_id = $1 AND id = $2",
    )
    .bind(acme)
    .bind(joined.id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!((member_id, client_id), (Some(bot), None));
}

#[sqlx::test(migrations = false)]
async fn a_sent_text_is_stored_as_a_text_message_of_its_sender(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let acme = tenant(&pool, "Acme").await;
    let ann = client(&pool, acme, "Ann").await;
    let (messaging, _) = messaging(&pool);
    let thread = messaging
        .open(acme, Party::Client(ann), None)
        .await
        .unwrap();
    let speaker = messaging.speaker(acme, thread.id, ann).await.unwrap();

    let sent = messaging
        .send(acme, thread.id, speaker.id, Content::text("Which service?"))
        .await
        .unwrap();

    let stored: (String, Option<String>, Uuid, Uuid) = sqlx::query_as(
        "SELECT kind, text, sender_id, thread_id FROM messages WHERE tenant_id = $1 AND id = $2",
    )
    .bind(acme)
    .bind(sent.id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        stored,
        (
            "text".into(),
            Some("Which service?".into()),
            speaker.id,
            thread.id
        )
    );
}

#[sqlx::test(migrations = false)]
async fn messages_come_in_pages_that_together_hold_every_message_once(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let acme = tenant(&pool, "Acme").await;
    let ann = client(&pool, acme, "Ann").await;
    let (messaging, _) = messaging(&pool);
    let thread = messaging
        .open(acme, Party::Client(ann), None)
        .await
        .unwrap();
    let speaker = messaging.speaker(acme, thread.id, ann).await.unwrap();
    for n in 0..5 {
        messaging
            .send(
                acme,
                thread.id,
                speaker.id,
                Content::text(format!("message {n}")),
            )
            .await
            .unwrap();
    }

    let first = messaging
        .messages(
            acme,
            thread.id,
            PageRequest {
                page_size: Some(2),
                page_token: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(first.items.len(), 2);
    let second = messaging
        .messages(
            acme,
            thread.id,
            PageRequest {
                page_size: Some(2),
                page_token: first.next_page_token.clone(),
            },
        )
        .await
        .unwrap();
    assert_eq!(second.items.len(), 2);
    let third = messaging
        .messages(
            acme,
            thread.id,
            PageRequest {
                page_size: Some(2),
                page_token: second.next_page_token.clone(),
            },
        )
        .await
        .unwrap();

    let mut ids = [&first, &second, &third]
        .iter()
        .flat_map(|page| page.items.iter().map(|m| m.id))
        .collect::<Vec<_>>();
    ids.sort();
    ids.dedup();
    assert_eq!(
        (ids.len(), third.items.len(), third.next_page_token),
        (5, 1, None)
    );
}

#[sqlx::test(migrations = false)]
async fn a_page_with_more_messages_behind_it_carries_a_next_page_token(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let acme = tenant(&pool, "Acme").await;
    let ann = client(&pool, acme, "Ann").await;
    let (messaging, _) = messaging(&pool);
    let thread = messaging
        .open(acme, Party::Client(ann), None)
        .await
        .unwrap();
    let speaker = messaging.speaker(acme, thread.id, ann).await.unwrap();
    for n in 0..3 {
        messaging
            .send(
                acme,
                thread.id,
                speaker.id,
                Content::text(format!("message {n}")),
            )
            .await
            .unwrap();
    }

    let page = messaging
        .messages(
            acme,
            thread.id,
            PageRequest {
                page_size: Some(2),
                page_token: None,
            },
        )
        .await
        .unwrap();

    assert!(page.next_page_token.is_some());
}

#[sqlx::test(migrations = false)]
async fn another_tenant_cannot_read_a_thread_over_http(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let (acme, globex) = (tenant(&pool, "Acme").await, tenant(&pool, "Globex").await);
    let ann = client(&pool, acme, "Ann").await;
    let (messaging, _) = messaging(&pool);
    let thread = messaging
        .open(acme, Party::Client(ann), None)
        .await
        .unwrap();
    let globex_app = full_app(pool, messaging, globex, Uuid::now_v7());

    let (thread_status, _) = call(
        &globex_app,
        "GET",
        &format!("/api/v1/tenants/{globex}/threads/{}", thread.id),
        Value::Null,
    )
    .await;
    let (messages_status, _) = call(
        &globex_app,
        "GET",
        &format!("/api/v1/tenants/{globex}/threads/{}/messages", thread.id),
        Value::Null,
    )
    .await;

    assert_eq!(
        (thread_status, messages_status),
        (StatusCode::NOT_FOUND, StatusCode::NOT_FOUND)
    );
}

#[sqlx::test(migrations = false)]
async fn another_tenant_cannot_read_or_write_a_thread_through_the_interface(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let (acme, globex) = (tenant(&pool, "Acme").await, tenant(&pool, "Globex").await);
    let ann = client(&pool, acme, "Ann").await;
    let (messaging, _) = messaging(&pool);
    let thread = messaging
        .open(acme, Party::Client(ann), None)
        .await
        .unwrap();
    let speaker = messaging.speaker(acme, thread.id, ann).await.unwrap();

    let read = messaging
        .messages(
            globex,
            thread.id,
            PageRequest {
                page_size: None,
                page_token: None,
            },
        )
        .await;
    let write = messaging
        .send(globex, thread.id, speaker.id, Content::text("hi"))
        .await;

    assert!(matches!(read, Err(MessagingError::NotFound)));
    assert!(matches!(write, Err(MessagingError::NotFound)));
}

#[sqlx::test(migrations = false)]
async fn every_route_needs_its_grant_and_a_caller_without_it_gets_403(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let acme = tenant(&pool, "Acme").await;
    let ann = client(&pool, acme, "Ann").await;
    let (messaging, _) = messaging(&pool);
    let thread = messaging
        .open(acme, Party::Client(ann), None)
        .await
        .unwrap()
        .id;
    let label: Uuid = sqlx::query_scalar(
        "INSERT INTO labels (tenant_id, id, name) VALUES ($1, uuidv7(), 'vip') RETURNING id",
    )
    .bind(acme)
    .fetch_one(&pool)
    .await
    .unwrap();
    let base = format!("/api/v1/tenants/{acme}");
    let routes = [
        (
            "POST",
            format!("{base}/threads"),
            json!({"subject": "Hello"}),
            "threads:create",
        ),
        (
            "GET",
            format!("{base}/threads"),
            Value::Null,
            "threads:read",
        ),
        (
            "GET",
            format!("{base}/threads/{thread}"),
            Value::Null,
            "threads:read",
        ),
        (
            "POST",
            format!("{base}/threads/{thread}/participants"),
            json!({}),
            "participants:create",
        ),
        (
            "GET",
            format!("{base}/threads/{thread}/participants"),
            Value::Null,
            "participants:read",
        ),
        (
            "POST",
            format!("{base}/threads/{thread}/messages"),
            json!({"text": "hi"}),
            "messages:create",
        ),
        (
            "GET",
            format!("{base}/threads/{thread}/messages"),
            Value::Null,
            "messages:read",
        ),
        (
            "POST",
            format!(
                "{base}/threads/{thread}/messages/{}:respond",
                Uuid::now_v7()
            ),
            json!({"name": "submit", "context": {}}),
            "messages:respond",
        ),
        (
            "PUT",
            format!("{base}/threads/{thread}/labels/{label}"),
            Value::Null,
            "labels:update",
        ),
        (
            "POST",
            format!("{base}/threads/{thread}:seen"),
            json!({"up_to": Uuid::now_v7()}),
            "threads:seen",
        ),
        (
            "POST",
            format!("{base}/threads/{thread}/typing"),
            Value::Null,
            "typing:create",
        ),
        (
            "POST",
            format!("{base}/threads/{thread}/attachments?filename=a.txt"),
            Value::Null,
            "attachments:create",
        ),
        (
            "GET",
            format!("{base}/threads/{thread}/attachments"),
            Value::Null,
            "attachments:read",
        ),
    ];

    for (method, uri, body, needed) in routes {
        let tenant_grant = format!("tenants:read:{acme}");
        let parent = if needed == "threads:read" {
            vec![]
        } else {
            vec!["threads:read"]
        };
        let without = [vec![tenant_grant.as_str()], parent.clone()].concat();
        let with = [without.clone(), vec![needed]].concat();

        let (denied, _) = call(
            &app(pool.clone(), messaging.clone(), acme, ann, &without),
            method,
            &uri,
            body.clone(),
        )
        .await;
        let (allowed, _) = call(
            &app(pool.clone(), messaging.clone(), acme, ann, &with),
            method,
            &uri,
            body,
        )
        .await;

        assert_eq!(
            denied,
            StatusCode::FORBIDDEN,
            "{method} {uri} without {needed}"
        );
        assert_ne!(
            allowed,
            StatusCode::FORBIDDEN,
            "{method} {uri} with {needed}"
        );
    }
}

#[sqlx::test(migrations = false)]
async fn a_member_of_the_tenant_becomes_a_participant_when_it_first_speaks(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let acme = tenant(&pool, "Acme").await;
    let ann = client(&pool, acme, "Ann").await;
    let bot = agent(&pool, acme, "assistant").await;
    let (messaging, _) = messaging(&pool);
    let thread = messaging
        .open(acme, Party::Client(ann), None)
        .await
        .unwrap();

    let speaker = messaging.speaker(acme, thread.id, bot).await.unwrap();

    let member: Option<Uuid> =
        sqlx::query_scalar("SELECT member_id FROM participants WHERE tenant_id = $1 AND id = $2")
            .bind(acme)
            .bind(speaker.id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(member, Some(bot));
}

#[sqlx::test(migrations = false)]
async fn a_speaker_who_already_took_part_is_the_same_participant(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let acme = tenant(&pool, "Acme").await;
    let ann = client(&pool, acme, "Ann").await;
    let bot = agent(&pool, acme, "assistant").await;
    let (messaging, _) = messaging(&pool);
    let thread = messaging
        .open(acme, Party::Client(ann), None)
        .await
        .unwrap();
    let joined = messaging
        .join(acme, thread.id, Party::Member(bot))
        .await
        .unwrap();

    let first = messaging.speaker(acme, thread.id, bot).await.unwrap();
    let second = messaging.speaker(acme, thread.id, bot).await.unwrap();

    assert_eq!((first.id, second.id), (joined.id, joined.id));
}

#[sqlx::test(migrations = false)]
async fn a_speaker_that_is_neither_a_participant_nor_a_tenant_member_is_not_found(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let (acme, globex) = (tenant(&pool, "Acme").await, tenant(&pool, "Globex").await);
    let ann = client(&pool, acme, "Ann").await;
    let foreign = agent(&pool, globex, "assistant").await;
    let (messaging, _) = messaging(&pool);
    let thread = messaging
        .open(acme, Party::Client(ann), None)
        .await
        .unwrap();

    let stranger = messaging.speaker(acme, thread.id, Uuid::now_v7()).await;
    let other_tenants_member = messaging.speaker(acme, thread.id, foreign).await;

    assert!(matches!(stranger, Err(MessagingError::NotFound)));
    assert!(matches!(
        other_tenants_member,
        Err(MessagingError::NotFound)
    ));
}

#[sqlx::test(migrations = false)]
async fn posting_a_message_sends_it_as_the_caller_with_its_reply_link(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let acme = tenant(&pool, "Acme").await;
    let ann = client(&pool, acme, "Ann").await;
    let bot = agent(&pool, acme, "assistant").await;
    let (messaging, _) = messaging(&pool);
    let thread = messaging
        .open(acme, Party::Client(ann), None)
        .await
        .unwrap();
    let ann_speaks = messaging.speaker(acme, thread.id, ann).await.unwrap();
    let question = messaging
        .send(
            acme,
            thread.id,
            ann_speaks.id,
            Content::text("Which service?"),
        )
        .await
        .unwrap();
    let bot_app = full_app(pool, messaging.clone(), acme, bot);

    let (status, reply) = call(
        &bot_app,
        "POST",
        &format!("/api/v1/tenants/{acme}/threads/{}/messages", thread.id),
        json!({"text": "A haircut", "in_reply_to": question.id}),
    )
    .await;

    let bot_speaks = messaging.speaker(acme, thread.id, bot).await.unwrap();
    assert_eq!(status, StatusCode::OK, "{reply}");
    assert_eq!(
        (
            reply["kind"].clone(),
            reply["text"].clone(),
            reply["sender_id"].clone(),
            reply["in_reply_to"].clone(),
            reply["attachments"].clone()
        ),
        (
            json!("text"),
            json!("A haircut"),
            json!(bot_speaks.id),
            json!(question.id),
            json!([])
        )
    );
}

#[sqlx::test(migrations = false)]
async fn listing_messages_over_http_answers_a_page_of_message_json(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let acme = tenant(&pool, "Acme").await;
    let ann = client(&pool, acme, "Ann").await;
    let (messaging, _) = messaging(&pool);
    let thread = messaging
        .open(acme, Party::Client(ann), None)
        .await
        .unwrap();
    let speaker = messaging.speaker(acme, thread.id, ann).await.unwrap();
    let sent = messaging
        .send(acme, thread.id, speaker.id, Content::text("Hi"))
        .await
        .unwrap();
    let ann_app = full_app(pool, messaging, acme, ann);

    let (status, page) = call(
        &ann_app,
        "GET",
        &format!("/api/v1/tenants/{acme}/threads/{}/messages", thread.id),
        Value::Null,
    )
    .await;

    assert_eq!(status, StatusCode::OK, "{page}");
    assert_eq!(
        (
            page["items"][0]["id"].clone(),
            page["items"][0]["text"].clone()
        ),
        (json!(sent.id), json!("Hi"))
    );
}

#[sqlx::test(migrations = false)]
async fn a_message_is_published_on_its_threads_topic_with_the_message_id_as_event_id(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let acme = tenant(&pool, "Acme").await;
    let ann = client(&pool, acme, "Ann").await;
    let (messaging, broker) = messaging(&pool);
    let thread = messaging
        .open(acme, Party::Client(ann), None)
        .await
        .unwrap();
    let speaker = messaging.speaker(acme, thread.id, ann).await.unwrap();
    let mut events = watch(&broker, &[format!("tenants/{acme}/threads/{}", thread.id)]).await;

    let sent = messaging
        .send(acme, thread.id, speaker.id, Content::text("Hi"))
        .await
        .unwrap();

    let event = next(&mut events).await;
    assert_eq!(event.id, sent.id.to_string());
    assert_eq!(
        &*event.topic,
        format!("tenants/{acme}/threads/{}", thread.id)
    );
    assert_eq!(
        (
            event.data["kind"].clone(),
            event.data["message"]["id"].clone()
        ),
        (json!("message"), json!(sent.id))
    );
}

#[sqlx::test(migrations = false)]
async fn a_message_is_not_published_on_another_threads_topic(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let acme = tenant(&pool, "Acme").await;
    let (ann, bob) = (
        client(&pool, acme, "Ann").await,
        client(&pool, acme, "Bob").await,
    );
    let (messaging, broker) = messaging(&pool);
    let (anns, bobs) = (
        messaging
            .open(acme, Party::Client(ann), None)
            .await
            .unwrap(),
        messaging
            .open(acme, Party::Client(bob), None)
            .await
            .unwrap(),
    );
    let speaker = messaging.speaker(acme, bobs.id, bob).await.unwrap();
    let mut events = watch(&broker, &[format!("tenants/{acme}/threads/{}", anns.id)]).await;

    messaging
        .send(acme, bobs.id, speaker.id, Content::text("Hi"))
        .await
        .unwrap();

    assert!(
        tokio::time::timeout(Duration::from_millis(300), events.recv())
            .await
            .is_err()
    );
}

#[sqlx::test(migrations = false)]
async fn opening_a_thread_publishes_it_on_the_tenants_thread_list_topic(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let acme = tenant(&pool, "Acme").await;
    let ann = client(&pool, acme, "Ann").await;
    let (messaging, broker) = messaging(&pool);
    let mut events = watch(&broker, &[format!("tenants/{acme}/threads")]).await;

    let thread = messaging
        .open(acme, Party::Client(ann), None)
        .await
        .unwrap();

    let event = next(&mut events).await;
    assert_eq!(
        (
            event.data["kind"].clone(),
            event.data["thread"]["id"].clone()
        ),
        (json!("thread"), json!(thread.id))
    );
}

#[sqlx::test(migrations = false)]
async fn a_member_with_the_read_grants_may_follow_a_thread_and_the_thread_list(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let acme = tenant(&pool, "Acme").await;
    let thread = Uuid::now_v7();
    let allow = megh::messaging::authorizer();
    let member = AccessToken {
        scope: format!("tenants:read:{acme} threads:read"),
        ..token(Uuid::now_v7(), acme)
    };

    let one = allow(
        Some(&member),
        &Topic::new(format!("tenants/{acme}/threads/{thread}")),
    );
    let list = allow(
        Some(&member),
        &Topic::new(format!("tenants/{acme}/threads")),
    );

    assert!(one && list);
}

#[sqlx::test(migrations = false)]
async fn the_authorizer_refuses_other_topics_other_tenants_and_no_token(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let (acme, globex) = (tenant(&pool, "Acme").await, tenant(&pool, "Globex").await);
    let thread = Uuid::now_v7();
    let allow = megh::messaging::authorizer();
    let member = AccessToken {
        scope: format!("tenants:read:{acme} threads:read"),
        ..token(Uuid::now_v7(), acme)
    };

    let other_tenant = allow(
        Some(&member),
        &Topic::new(format!("tenants/{globex}/threads/{thread}")),
    );
    let elsewhere = allow(
        Some(&member),
        &Topic::new(format!("tenants/{acme}/invoices")),
    );
    let anonymous = allow(
        None,
        &Topic::new(format!("tenants/{acme}/threads/{thread}")),
    );

    assert_eq!((other_tenant, elsewhere, anonymous), (false, false, false));
}
