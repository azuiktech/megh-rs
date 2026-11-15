//! Messaging guests (docs/TPD-messaging.md): `POST /tenants/{t}/guests` creates a guest client with a thread, a
//! participant and a token that reaches that one thread only, in HTTP and in events.

use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::middleware::{from_fn, from_fn_with_state};
use axum::{Extension, Router};
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode};
use megh::auth::rest::AppContext;
use megh::auth::{AccessToken, JwtSession, jwt_session};
use megh::messaging::{Content, Messaging, Party};
use megh::{Broker, Topic, authorizer};
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::util::MapRequestLayer;
use tower::{Layer, ServiceExt};
use tower_sessions::{MemoryStore, SessionManagerLayer};
use uuid::Uuid;

const SECRET: &[u8] = b"messaging-guest-test-secret-32-b";

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

fn jwt(pool: &PgPool) -> JwtSession {
    JwtSession::new(SECRET, Duration::from_secs(300), pool.clone())
}

fn app(pool: PgPool, messaging: Messaging, jwt: JwtSession) -> Router {
    let (guarded, _) = megh::messaging::router().split_for_parts();
    let (public, _) = megh::messaging::guest_router().split_for_parts();
    let guarded = guarded
        .layer(from_fn(authorizer))
        .layer(from_fn_with_state(jwt.clone(), jwt_session));
    Router::new()
        .nest("/api/v1", public.merge(guarded))
        .layer(Extension(messaging))
        .layer(Extension(jwt))
        .layer(SessionManagerLayer::new(MemoryStore::default()))
        .with_state(AppContext { pool })
}

async fn call(
    app: &Router,
    method: &str,
    uri: &str,
    bearer: Option<&str>,
    body: Value,
) -> (StatusCode, Value) {
    let request = Request::builder().method(method).uri(uri);
    let request = match bearer {
        Some(token) => request.header("authorization", format!("Bearer {token}")),
        None => request,
    };
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

struct Fixture {
    app: Router,
    messaging: Messaging,
}

fn fixture(pool: PgPool) -> Fixture {
    let messaging = Messaging::new(pool.clone(), Broker::new());
    Fixture {
        app: app(pool.clone(), messaging.clone(), jwt(&pool)),
        messaging,
    }
}

async fn arrive(fixture: &Fixture, tenant: Uuid) -> Value {
    let (status, guest) = call(
        &fixture.app,
        "POST",
        &format!("/api/v1/tenants/{tenant}/guests"),
        None,
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{guest}");
    guest
}

fn id(value: &Value) -> Uuid {
    value.as_str().unwrap().parse().unwrap()
}

fn bearer(guest: &Value) -> &str {
    guest["token"].as_str().unwrap()
}

fn messages_uri(tenant: Uuid, thread: &str) -> String {
    format!("/api/v1/tenants/{tenant}/threads/{thread}/messages")
}

fn claims(guest: &Value) -> AccessToken {
    let mut validation = Validation::new(Algorithm::HS256);
    validation.validate_aud = false;
    decode::<AccessToken>(
        bearer(guest),
        &DecodingKey::from_secret(SECRET),
        &validation,
    )
    .unwrap()
    .claims
}

#[sqlx::test(migrations = false)]
async fn a_guest_arrives_with_a_client_a_thread_a_participant_and_a_token(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let acme = tenant(&pool, "Acme").await;
    let fixture = fixture(pool);

    let guest = arrive(&fixture, acme).await;

    assert_eq!(guest["client"]["kind"], json!("guest"));
    assert_eq!(guest["participant"]["client_id"], guest["client"]["id"]);
    assert!(
        guest["thread"]["id"].is_string()
            && guest["token"].is_string()
            && !guest["expires_in"].is_null(),
        "{guest}"
    );
}

#[sqlx::test(migrations = false)]
async fn a_guest_has_placeholder_names_made_from_the_start_of_its_id(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let acme = tenant(&pool, "Acme").await;
    let fixture = fixture(pool);

    let guest = arrive(&fixture, acme).await;

    let four = guest["client"]["id"].as_str().unwrap().replace('-', "")[28..].to_string();
    assert!(four.chars().all(|c| c.is_ascii_hexdigit()));
    assert_eq!(
        (
            guest["client"]["first_name"].clone(),
            guest["client"]["last_name"].clone(),
            guest["client"]["name"].clone()
        ),
        (json!("Guest"), json!(four), json!(format!("Guest {four}")))
    );
}

#[sqlx::test(migrations = false)]
async fn the_guest_client_and_its_thread_are_stored_in_the_tenant(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let acme = tenant(&pool, "Acme").await;
    let fixture = fixture(pool.clone());

    let guest = arrive(&fixture, acme).await;

    let kind: String =
        sqlx::query_scalar("SELECT kind FROM clients WHERE tenant_id = $1 AND id = $2")
            .bind(acme)
            .bind(id(&guest["client"]["id"]))
            .fetch_one(&pool)
            .await
            .unwrap();
    let in_thread: i64 = sqlx::query_scalar("SELECT count(*) FROM participants WHERE tenant_id = $1 AND thread_id = $2 AND client_id = $3")
        .bind(acme)
        .bind(id(&guest["thread"]["id"]))
        .bind(id(&guest["client"]["id"]))
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!((kind.as_str(), in_thread), ("guest", 1));
}

#[sqlx::test(migrations = false)]
async fn each_arrival_is_a_new_guest_with_its_own_thread(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let acme = tenant(&pool, "Acme").await;
    let fixture = fixture(pool);

    let first = arrive(&fixture, acme).await;
    let second = arrive(&fixture, acme).await;

    assert_ne!(first["client"]["id"], second["client"]["id"]);
    assert_ne!(first["thread"]["id"], second["thread"]["id"]);
}

#[sqlx::test(migrations = false)]
async fn the_guest_token_names_the_client_the_tenant_and_the_instance_grants(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let acme = tenant(&pool, "Acme").await;
    let fixture = fixture(pool);
    let guest = arrive(&fixture, acme).await;
    let thread = guest["thread"]["id"].as_str().unwrap();

    let token = claims(&guest);

    let mut scope = token
        .scope
        .split_whitespace()
        .map(String::from)
        .collect::<Vec<_>>();
    let mut expected = [
        format!("tenants:read:{acme}"),
        format!("threads:read:{thread}"),
        format!("threads:seen:{thread}"),
        "messages:create".into(),
        "messages:read".into(),
        "messages:respond".into(),
        "participants:read".into(),
        "attachments:create".into(),
        "attachments:read".into(),
        "typing:create".into(),
    ]
    .to_vec();
    scope.sort();
    expected.sort();
    assert_eq!(
        (token.sub.to_string(), token.aud, scope),
        (
            guest["client"]["id"].as_str().unwrap().to_string(),
            vec![acme.to_string()],
            expected
        )
    );
}

#[sqlx::test(migrations = false)]
async fn a_guest_sends_and_reads_messages_in_its_own_thread(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let acme = tenant(&pool, "Acme").await;
    let fixture = fixture(pool);
    let guest = arrive(&fixture, acme).await;
    let uri = messages_uri(acme, guest["thread"]["id"].as_str().unwrap());

    let (sent_status, sent) = call(
        &fixture.app,
        "POST",
        &uri,
        Some(bearer(&guest)),
        json!({"text": "Do you have a slot?"}),
    )
    .await;
    let (read_status, page) =
        call(&fixture.app, "GET", &uri, Some(bearer(&guest)), Value::Null).await;

    assert_eq!(
        (sent_status, read_status),
        (StatusCode::OK, StatusCode::OK),
        "{sent} {page}"
    );
    assert_eq!(sent["sender_id"], guest["participant"]["id"]);
    assert_eq!(page["items"][0]["text"], json!("Do you have a slot?"));
}

#[sqlx::test(migrations = false)]
async fn a_guest_sees_what_the_agent_said_in_its_thread(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let acme = tenant(&pool, "Acme").await;
    let fixture = fixture(pool.clone());
    let guest = arrive(&fixture, acme).await;
    let thread = id(&guest["thread"]["id"]);
    let agent = megh::UserRepo::new(&pool)
        .machine(acme, "assistant", vec![format!("tenants:read:{acme}")])
        .await
        .unwrap();
    let speaker = fixture
        .messaging
        .join(acme, thread, Party::Member(agent.id))
        .await
        .unwrap();
    fixture
        .messaging
        .send(acme, thread, speaker.id, Content::text("Which service?"))
        .await
        .unwrap();

    let (status, page) = call(
        &fixture.app,
        "GET",
        &messages_uri(acme, &thread.to_string()),
        Some(bearer(&guest)),
        Value::Null,
    )
    .await;

    assert_eq!(
        (status, page["items"][0]["text"].clone()),
        (StatusCode::OK, json!("Which service?"))
    );
}

#[sqlx::test(migrations = false)]
async fn a_guest_is_refused_another_thread_of_its_tenant(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let acme = tenant(&pool, "Acme").await;
    let ann = client(&pool, acme, "Ann").await;
    let fixture = fixture(pool);
    let guest = arrive(&fixture, acme).await;
    let other = fixture
        .messaging
        .open(acme, Party::Client(ann), None)
        .await
        .unwrap()
        .id
        .to_string();

    let (read, _) = call(
        &fixture.app,
        "GET",
        &messages_uri(acme, &other),
        Some(bearer(&guest)),
        Value::Null,
    )
    .await;
    let (write, _) = call(
        &fixture.app,
        "POST",
        &messages_uri(acme, &other),
        Some(bearer(&guest)),
        json!({"text": "Hello?"}),
    )
    .await;

    assert_eq!(
        (read, write),
        (StatusCode::FORBIDDEN, StatusCode::FORBIDDEN)
    );
}

#[sqlx::test(migrations = false)]
async fn a_guest_is_refused_another_tenant(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let (acme, globex) = (tenant(&pool, "Acme").await, tenant(&pool, "Globex").await);
    let bob = client(&pool, globex, "Bob").await;
    let fixture = fixture(pool);
    let guest = arrive(&fixture, acme).await;
    let theirs = fixture
        .messaging
        .open(globex, Party::Client(bob), None)
        .await
        .unwrap()
        .id
        .to_string();

    let (status, _) = call(
        &fixture.app,
        "GET",
        &messages_uri(globex, &theirs),
        Some(bearer(&guest)),
        Value::Null,
    )
    .await;

    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[sqlx::test(migrations = false)]
async fn a_guest_is_refused_the_labels_and_the_thread_list(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let acme = tenant(&pool, "Acme").await;
    let fixture = fixture(pool);
    let guest = arrive(&fixture, acme).await;

    let (labels, _) = call(
        &fixture.app,
        "GET",
        &format!("/api/v1/tenants/{acme}/labels"),
        Some(bearer(&guest)),
        Value::Null,
    )
    .await;
    let (threads, _) = call(
        &fixture.app,
        "GET",
        &format!("/api/v1/tenants/{acme}/threads"),
        Some(bearer(&guest)),
        Value::Null,
    )
    .await;

    assert_eq!(
        (labels, threads),
        (StatusCode::FORBIDDEN, StatusCode::FORBIDDEN)
    );
}

#[sqlx::test(migrations = false)]
async fn a_guest_may_read_its_thread_and_mark_it_seen(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let acme = tenant(&pool, "Acme").await;
    let fixture = fixture(pool);
    let guest = arrive(&fixture, acme).await;
    let thread = guest["thread"]["id"].as_str().unwrap();
    let (_, sent) = call(
        &fixture.app,
        "POST",
        &messages_uri(acme, thread),
        Some(bearer(&guest)),
        json!({"text": "Hi"}),
    )
    .await;

    let (read, _) = call(
        &fixture.app,
        "GET",
        &format!("/api/v1/tenants/{acme}/threads/{thread}"),
        Some(bearer(&guest)),
        Value::Null,
    )
    .await;
    let (seen, body) = call(
        &fixture.app,
        "POST",
        &format!("/api/v1/tenants/{acme}/threads/{thread}:seen"),
        Some(bearer(&guest)),
        json!({"up_to": sent["id"]}),
    )
    .await;

    assert_eq!((read, seen), (StatusCode::OK, StatusCode::OK), "{body}");
}

#[sqlx::test(migrations = false)]
async fn a_request_without_a_token_is_not_served(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let acme = tenant(&pool, "Acme").await;
    let fixture = fixture(pool);
    let guest = arrive(&fixture, acme).await;

    let (status, _) = call(
        &fixture.app,
        "GET",
        &messages_uri(acme, guest["thread"]["id"].as_str().unwrap()),
        None,
        Value::Null,
    )
    .await;

    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[sqlx::test(migrations = false)]
async fn the_guest_token_may_follow_its_own_thread_topic(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let acme = tenant(&pool, "Acme").await;
    let fixture = fixture(pool);
    let guest = arrive(&fixture, acme).await;
    let allow = megh::messaging::authorizer();

    let own = allow(
        Some(&claims(&guest)),
        &Topic::new(format!(
            "tenants/{acme}/threads/{}",
            guest["thread"]["id"].as_str().unwrap()
        )),
    );

    assert!(own);
}

#[sqlx::test(migrations = false)]
async fn the_guest_token_may_not_follow_another_thread_topic(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let acme = tenant(&pool, "Acme").await;
    let fixture = fixture(pool);
    let guest = arrive(&fixture, acme).await;
    let other = arrive(&fixture, acme).await;
    let allow = megh::messaging::authorizer();

    let theirs = allow(
        Some(&claims(&guest)),
        &Topic::new(format!(
            "tenants/{acme}/threads/{}",
            other["thread"]["id"].as_str().unwrap()
        )),
    );

    assert!(!theirs);
}

#[sqlx::test(migrations = false)]
async fn the_guest_token_may_not_follow_the_thread_list_or_a_pattern(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let acme = tenant(&pool, "Acme").await;
    let fixture = fixture(pool);
    let guest = arrive(&fixture, acme).await;
    let allow = megh::messaging::authorizer();
    let token = claims(&guest);

    let list = allow(Some(&token), &Topic::new(format!("tenants/{acme}/threads")));
    let any_thread = allow(
        Some(&token),
        &Topic::new(format!("tenants/{acme}/threads/*")),
    );
    let everything = allow(Some(&token), &Topic::new(format!("tenants/{acme}/**")));

    assert_eq!((list, any_thread, everything), (false, false, false));
}

#[sqlx::test(migrations = false)]
async fn the_authorizer_allows_nothing_without_a_token(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let acme = tenant(&pool, "Acme").await;
    let fixture = fixture(pool);
    let guest = arrive(&fixture, acme).await;
    let allow = megh::messaging::authorizer();

    let anonymous = allow(
        None,
        &Topic::new(format!(
            "tenants/{acme}/threads/{}",
            guest["thread"]["id"].as_str().unwrap()
        )),
    );

    assert!(!anonymous);
}
