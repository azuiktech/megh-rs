//! `Encryptor` wired into `ConnectedAccountRepo` and `Accounts`: tokens are ciphertext in the database and
//! plaintext everywhere else.

use std::sync::Arc;

use axum::http::{header::AUTHORIZATION, HeaderMap, StatusCode};
use axum::routing::{get, post};
use axum::{Json, Router};
use megh::{Accounts, ConnectedAccount, ConnectedAccountRepo, Encryptor, OAuthProviderConfig};
use serde_json::json;
use sqlx::PgPool;
use std::collections::HashMap;

const KEY: [u8; 32] = [3u8; 32];

fn account() -> ConnectedAccount {
    ConnectedAccount {
        account_id: "acct".into(),
        provider: "google".into(),
        email: Some("ann@example.com".into()),
        access_token: "plain-access".into(),
        refresh_token: Some("plain-refresh".into()),
        token_type: Some("Bearer".into()),
        expiry: None,
        created_at: None,
        updated_at: None,
        disconnected_at: None,
    }
}

async fn stored_tokens(pool: &PgPool) -> (String, String) {
    sqlx::query_as("SELECT access_token, refresh_token FROM connected_accounts WHERE account_id = 'acct'")
        .fetch_one(pool)
        .await
        .unwrap()
}

#[sqlx::test]
async fn saved_tokens_are_ciphertext_in_the_database_and_plaintext_from_the_repo(pool: PgPool) {
    let repo = ConnectedAccountRepo::new(&pool).with_encryptor(Encryptor::new(&KEY));

    let saved = repo.save(&account()).await.unwrap();

    assert_eq!((saved.access_token.as_str(), saved.refresh_token.as_deref()), ("plain-access", Some("plain-refresh")));
    let (stored_access, stored_refresh) = stored_tokens(&pool).await;
    assert_ne!(stored_access, "plain-access");
    assert_ne!(stored_refresh, "plain-refresh");
}

#[sqlx::test]
async fn get_find_and_list_decrypt(pool: PgPool) {
    let repo = ConnectedAccountRepo::new(&pool).with_encryptor(Encryptor::new(&KEY));
    repo.save(&account()).await.unwrap();

    let by_id = repo.get("acct", "google").await.unwrap().unwrap();
    let by_email = repo.find_by_email_or_account("ann@example.com", "google").await.unwrap().unwrap();
    let listed = repo.list_by_email("ann@example.com").await.unwrap();

    assert_eq!(by_id.access_token, "plain-access");
    assert_eq!(by_email.refresh_token.as_deref(), Some("plain-refresh"));
    assert_eq!(listed[0].access_token, "plain-access");
}

#[sqlx::test]
async fn without_an_encryptor_tokens_stay_plaintext(pool: PgPool) {
    let repo = ConnectedAccountRepo::new(&pool);

    repo.save(&account()).await.unwrap();

    let (stored_access, _) = stored_tokens(&pool).await;
    assert_eq!(stored_access, "plain-access");
}

async fn stub_provider(reply: serde_json::Value) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let app = Router::new()
        .route("/token", post(move || async move { (StatusCode::OK, Json(reply)) }))
        .route("/api", get(|headers: HeaderMap| async move { headers[AUTHORIZATION].to_str().unwrap().to_string() }));
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    base
}

#[sqlx::test]
async fn a_refreshed_token_is_stored_encrypted_and_served_decrypted(pool: PgPool) {
    let base = stub_provider(json!({"access_token": "fresh-access", "token_type": "bearer", "expires_in": 3600, "refresh_token": "fresh-refresh"})).await;
    sqlx::query("INSERT INTO connected_accounts (account_id, provider, access_token, refresh_token, expiry) VALUES ('acct', 'google', $1, $2, NOW() - INTERVAL '1 second')")
        .bind(Encryptor::new(&KEY).encrypt("stale-access").unwrap())
        .bind(Encryptor::new(&KEY).encrypt("stale-refresh").unwrap())
        .execute(&pool)
        .await
        .unwrap();
    let providers = Arc::new(HashMap::from([(
        "google".to_string(),
        OAuthProviderConfig { provider_id: "google".into(), client_id: "id".into(), client_secret: None, auth_url: format!("{base}/authorize"), token_url: format!("{base}/token"), userinfo_url: None, default_scopes: vec![], redirect_url: None },
    )]));
    let accounts = Accounts::new(pool.clone(), providers).with_encryptor(Encryptor::new(&KEY));
    let client = reqwest_middleware::ClientBuilder::new(reqwest::Client::new()).with(accounts.auth("google", "acct")).build();

    let response = client.get(format!("{base}/api")).send().await.unwrap();

    assert_eq!(response.text().await.unwrap(), "Bearer fresh-access");
    let (stored_access, stored_refresh) = stored_tokens(&pool).await;
    assert!(stored_access != "fresh-access" && stored_refresh != "fresh-refresh");
    assert_eq!(Encryptor::new(&KEY).decrypt(&stored_access).unwrap(), "fresh-access");
}
