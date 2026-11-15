//! Messaging history (docs/TPD-messaging.md): `History::new(pool)` is the broker's history provider, so a client that
//! reconnects with `Last-Event-ID` receives the messages of its thread it missed, even after the ring buffer has
//! forgotten them.

use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::body::{Body, BodyDataStream};
use axum::http::Request;
use megh::booking::value::Name;
use megh::messaging::{Content, History, Messaging, Party};
use megh::{Broker, events_router};
use sqlx::PgPool;
use tokio_stream::StreamExt;
use tower::ServiceExt;
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

struct Talk {
    tenant: Uuid,
    thread: Uuid,
    speaker: Uuid,
}

async fn talk(pool: &PgPool, messaging: &Messaging, tenant: Uuid, who: &str) -> Talk {
    let ann = client(pool, tenant, who).await;
    let thread = messaging
        .open(
            tenant,
            Party::Client(ann),
            Some(Name::try_new(who).unwrap()),
        )
        .await
        .unwrap()
        .id;
    let speaker = messaging.speaker(tenant, thread, ann).await.unwrap().id;
    Talk {
        tenant,
        thread,
        speaker,
    }
}

async fn say(messaging: &Messaging, talk: &Talk, text: &str) -> Uuid {
    messaging
        .send(talk.tenant, talk.thread, talk.speaker, Content::text(text))
        .await
        .unwrap()
        .id
}

fn forgetful(pool: &PgPool) -> (Arc<Broker>, Messaging) {
    let broker = Broker::new()
        .with_history_provider(History::new(pool.clone()))
        .with_replay_capacity(1);
    let messaging = Messaging::new(pool.clone(), broker.clone());
    (Arc::new(broker), messaging)
}

async fn reconnect(app: &Router, talk: &Talk, last_event_id: Uuid) -> Vec<Uuid> {
    let request = Request::get(format!(
        "/stream?topic=tenants/{}/threads/{}",
        talk.tenant, talk.thread
    ))
    .header("last-event-id", last_event_id.to_string())
    .body(Body::empty())
    .unwrap();
    let mut stream: BodyDataStream = app
        .clone()
        .oneshot(request)
        .await
        .unwrap()
        .into_body()
        .into_data_stream();
    let mut text = String::new();
    while let Ok(Some(Ok(chunk))) =
        tokio::time::timeout(Duration::from_millis(500), stream.next()).await
    {
        text.push_str(&String::from_utf8_lossy(&chunk));
    }
    text.lines()
        .filter_map(|line| line.strip_prefix("id: "))
        .filter_map(|id| id.trim().parse().ok())
        .collect()
}

#[sqlx::test(migrations = false)]
async fn a_reconnecting_client_receives_every_later_message_of_its_thread_in_order(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let acme = tenant(&pool, "Acme").await;
    let (broker, messaging) = forgetful(&pool);
    let ann = talk(&pool, &messaging, acme, "Ann").await;
    let first = say(&messaging, &ann, "one").await;
    let second = say(&messaging, &ann, "two").await;
    let third = say(&messaging, &ann, "three").await;
    let fourth = say(&messaging, &ann, "four").await;
    let app = events_router(broker);

    let caught_up = reconnect(&app, &ann, first).await;

    assert_eq!(caught_up, [second, third, fourth]);
}

#[sqlx::test(migrations = false)]
async fn catch_up_leaves_out_the_messages_of_other_threads(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let acme = tenant(&pool, "Acme").await;
    let (broker, messaging) = forgetful(&pool);
    let (ann, bob) = (
        talk(&pool, &messaging, acme, "Ann").await,
        talk(&pool, &messaging, acme, "Bob").await,
    );
    let first = say(&messaging, &ann, "one").await;
    say(&messaging, &bob, "bob one").await;
    let second = say(&messaging, &ann, "two").await;
    say(&messaging, &bob, "bob two").await;
    let third = say(&messaging, &ann, "three").await;
    say(&messaging, &bob, "bob three").await;
    let app = events_router(broker);

    let caught_up = reconnect(&app, &ann, first).await;

    assert_eq!(caught_up, [second, third]);
}

#[sqlx::test(migrations = false)]
async fn catch_up_leaves_out_the_messages_of_another_tenants_thread(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let (acme, globex) = (tenant(&pool, "Acme").await, tenant(&pool, "Globex").await);
    let (broker, messaging) = forgetful(&pool);
    let (ann, bob) = (
        talk(&pool, &messaging, acme, "Ann").await,
        talk(&pool, &messaging, globex, "Bob").await,
    );
    let first = say(&messaging, &ann, "one").await;
    say(&messaging, &bob, "bob one").await;
    let second = say(&messaging, &ann, "two").await;
    let app = events_router(broker);

    let caught_up = reconnect(&app, &ann, first).await;

    assert_eq!(caught_up, [second]);
}

#[sqlx::test(migrations = false)]
async fn a_client_that_already_has_the_latest_message_receives_nothing_more(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let acme = tenant(&pool, "Acme").await;
    let (broker, messaging) = forgetful(&pool);
    let ann = talk(&pool, &messaging, acme, "Ann").await;
    say(&messaging, &ann, "one").await;
    let latest = say(&messaging, &ann, "two").await;
    let app = events_router(broker);

    let caught_up = reconnect(&app, &ann, latest).await;

    assert_eq!(caught_up, Vec::<Uuid>::new());
}
