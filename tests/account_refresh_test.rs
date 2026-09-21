use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::State;
use axum::http::{header::AUTHORIZATION, HeaderMap, StatusCode};
use axum::routing::{get, post};
use axum::{Json, Router};
use megh::{AccountError, Accounts, ConnectedAccount, ConnectedAccountRepo, OAuthProviderConfig};
use reqwest_middleware::ClientWithMiddleware;
use serde_json::{json, Value};
use sqlx::PgPool;

#[derive(Clone)]
struct Provider {
    token_calls: Arc<AtomicUsize>,
    reply: Arc<Mutex<(StatusCode, Value)>>,
    delay: Duration,
    base: String,
}

async fn provider(reply: (StatusCode, Value), delay: Duration) -> Provider {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let stub = Provider { token_calls: Arc::default(), reply: Arc::new(Mutex::new(reply)), delay, base };
    let app = Router::new()
        .route("/token", post(|State(p): State<Provider>| async move {
            p.token_calls.fetch_add(1, Ordering::SeqCst);
            tokio::time::sleep(p.delay).await;
            let (status, body) = p.reply.lock().unwrap().clone();
            (status, Json(body))
        }))
        .route("/api", get(|headers: HeaderMap| async move { headers[AUTHORIZATION].to_str().unwrap().to_string() }))
        .with_state(stub.clone());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    stub
}

fn fresh_token() -> (StatusCode, Value) {
    (StatusCode::OK, json!({"access_token": "fresh", "token_type": "bearer", "expires_in": 3600}))
}

async fn account(pool: &PgPool, expires_in_secs: i64) {
    sqlx::query("INSERT INTO connected_accounts (account_id, provider, access_token, refresh_token, expiry) VALUES ('acct', 'google', 'stored', 'old-refresh', NOW() + $1 * INTERVAL '1 second')")
        .bind(expires_in_secs)
        .execute(pool)
        .await
        .unwrap();
}

fn client(pool: &PgPool, stub: &Provider) -> ClientWithMiddleware {
    let mut google = OAuthProviderConfig::google("client-id", Some("secret".into()));
    google.token_url = format!("{}/token", stub.base);
    let accounts = Accounts::new(pool.clone(), Arc::new([("google".to_string(), google)].into()));
    reqwest_middleware::ClientBuilder::new(reqwest::Client::new()).with(accounts.auth("google", "acct")).build()
}

async fn call_api(client: &ClientWithMiddleware, stub: &Provider) -> reqwest_middleware::Result<String> {
    Ok(client.get(format!("{}/api", stub.base)).send().await?.text().await?)
}

async fn stored(pool: &PgPool) -> (String, Option<String>) {
    sqlx::query_as("SELECT access_token, refresh_token FROM connected_accounts WHERE account_id = 'acct'").fetch_one(pool).await.unwrap()
}

#[sqlx::test]
async fn valid_token_is_used_without_calling_the_provider(pool: PgPool) {
    let stub = provider(fresh_token(), Duration::ZERO).await;
    account(&pool, 3600).await;

    let body = call_api(&client(&pool, &stub), &stub).await.unwrap();

    assert_eq!((body.as_str(), stub.token_calls.load(Ordering::SeqCst)), ("Bearer stored", 0));
}

#[sqlx::test]
async fn expired_token_is_refreshed_once_and_stored(pool: PgPool) {
    let stub = provider(fresh_token(), Duration::ZERO).await;
    account(&pool, -3600).await;
    let api = client(&pool, &stub);

    assert_eq!(call_api(&api, &stub).await.unwrap(), "Bearer fresh");
    assert_eq!(call_api(&api, &stub).await.unwrap(), "Bearer fresh");

    assert_eq!(stub.token_calls.load(Ordering::SeqCst), 1);
    assert_eq!(stored(&pool).await, ("fresh".to_string(), Some("old-refresh".to_string())));
}

#[sqlx::test]
async fn a_rotated_refresh_token_is_stored(pool: PgPool) {
    let stub = provider((StatusCode::OK, json!({"access_token": "fresh", "token_type": "bearer", "expires_in": 3600, "refresh_token": "rotated"})), Duration::ZERO).await;
    account(&pool, -3600).await;

    call_api(&client(&pool, &stub), &stub).await.unwrap();

    assert_eq!(stored(&pool).await.1.as_deref(), Some("rotated"));
}

#[sqlx::test]
async fn concurrent_requests_refresh_once(pool: PgPool) {
    let stub = provider(fresh_token(), Duration::from_millis(300)).await;
    account(&pool, -3600).await;
    let api = client(&pool, &stub);

    let (first, second) = tokio::join!(call_api(&api, &stub), call_api(&api, &stub));

    assert_eq!((first.unwrap().as_str(), second.unwrap().as_str(), stub.token_calls.load(Ordering::SeqCst)), ("Bearer fresh", "Bearer fresh", 1));
}

#[sqlx::test]
async fn a_revoked_refresh_token_disconnects_the_account(pool: PgPool) {
    let stub = provider((StatusCode::BAD_REQUEST, json!({"error": "invalid_grant", "error_description": "revoked"})), Duration::ZERO).await;
    account(&pool, -3600).await;
    let api = client(&pool, &stub);

    let refused = call_api(&api, &stub).await.unwrap_err();
    let again = call_api(&api, &stub).await.unwrap_err();

    assert!(matches!(&refused, reqwest_middleware::Error::Middleware(e) if matches!(e.downcast_ref::<AccountError>(), Some(AccountError::InvalidGrant(_)))));
    assert!(matches!(&again, reqwest_middleware::Error::Middleware(e) if matches!(e.downcast_ref::<AccountError>(), Some(AccountError::Disconnected))));
    let disconnected: bool = sqlx::query_scalar("SELECT disconnected_at IS NOT NULL FROM connected_accounts WHERE account_id = 'acct'").fetch_one(&pool).await.unwrap();
    assert_eq!((disconnected, stub.token_calls.load(Ordering::SeqCst)), (true, 1));
}

#[sqlx::test]
async fn saving_a_login_without_a_refresh_token_keeps_the_stored_one(pool: PgPool) {
    account(&pool, 3600).await;
    let repo = ConnectedAccountRepo::new(&pool);
    let login = ConnectedAccount {
        account_id: "acct".into(),
        provider: "google".into(),
        email: Some("ann@example.com".into()),
        access_token: "new-access".into(),
        refresh_token: None,
        token_type: Some("Bearer".into()),
        expiry: None,
        created_at: None,
        updated_at: None,
        disconnected_at: None,
    };

    repo.save(&login).await.unwrap();

    assert_eq!(stored(&pool).await, ("new-access".to_string(), Some("old-refresh".to_string())));
}
