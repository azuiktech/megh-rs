//! Specification: a tenant's connections (issue #234).
//!
//! - `connections_router(state)` serves, under `/tenants/{tenant_id}` and behind `authorizer`:
//!   - `GET /connections`: the tenant's accounts, known (`/auth/{provider}/connect`) and generic, connected or not,
//!     as `Connection`s: `id`, `provider`, `account_id`, `email`, `url`, `icon_url`, `auth`, `key_header`,
//!     `connected`, `created_at`. Never a token. A deleted account and another tenant's are not listed.
//!   - `POST /connections` with `{url, bearer: {token}}` or `{url, api_key: {key, header?}}`: a generic connection,
//!     `provider` `url` and `account_id` its own `id`, so tenants connecting the same URL keep their own; an API key is sent in `X-API-Key` unless `header` says otherwise.
//!     An optional `spec_url` is its OpenAPI document, kept through reconnects. Its icon is the site's `<link rel="icon">`, made absolute, else `{origin}/favicon.ico`. A body without an
//!     `http(s)` URL, with a `spec_url` that is not one, or without exactly one of `bearer` and `api_key`, is refused (`400`).
//!   - `POST /connections/{id}:disconnect`: disconnects it; it is still listed, `connected: false`.
//!   - `PUT /connections/{id}` with `{bearer}` or `{api_key}`: connects it again with new credentials.
//!   - `DELETE /connections/{id}`: disconnects and deletes it (`204`); it is no longer listed.
//!   - Another tenant's connection is not found (`404`) by any of these.
//! - `Connections::secret(tenant_id, id)` is how the app calls the connection: `Secret::Bearer(token)` or
//!   `Secret::ApiKey { header, key }`. Tokens are ciphertext in the database when an encryptor is set. An expired
//!   OAuth token of a known provider (`with_providers`) is refreshed first.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::middleware::from_fn;
use axum::response::Html;
use axum::routing::get;
use axum::{Extension, Router};
use megh::account::{Connections, Secret};
use megh::{authorizer, connections_router, Encryptor, Grant, MeghAuthState};
use serde_json::{json, Value};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

const KEY: [u8; 32] = [7u8; 32];

async fn tenant(pool: &PgPool) -> Uuid {
    sqlx::query_scalar("INSERT INTO tenants (name) VALUES ('Shop') RETURNING id").fetch_one(pool).await.unwrap()
}

fn grants(tenant: Uuid) -> Vec<Grant> {
    [format!("connections:*"), format!("tenants:read:{tenant}")].iter().map(Grant::new).collect()
}

fn app(pool: &PgPool, grants: Vec<Grant>) -> Router {
    let state = MeghAuthState::new(pool.clone()).with_token_encryptor(Encryptor::new(&KEY));
    Router::new().nest("/api/v1", connections_router(state).layer(from_fn(authorizer))).layer(Extension(grants))
}

async fn call(app: &Router, method: &str, uri: &str, body: Value) -> (StatusCode, Value) {
    let request = Request::builder().method(method).uri(uri);
    let request = match body {
        Value::Null => request.body(Body::empty()),
        body => request.header("content-type", "application/json").body(Body::from(body.to_string())),
    };
    let response = app.clone().oneshot(megh::booking::rewrite_aip_136(request.unwrap())).await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

/// A site on a local port serving `page` at `/`; its origin.
async fn site(page: &'static str) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let router = Router::new().route("/", get(move || async move { Html(page) }));
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    origin
}

fn connections(pool: &PgPool) -> Connections {
    Connections::new(pool.clone()).with_encryptor(Encryptor::new(&KEY))
}

#[sqlx::test(migrations = false)]
async fn a_bearer_connection_is_stored_for_the_tenant_and_listed_without_its_token(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let mine = tenant(&pool).await;
    let app = app(&pool, grants(mine));
    let url = site("<html><head><title>CRM</title></head></html>").await;

    let (status, created) = call(&app, "POST", &format!("/api/v1/tenants/{mine}/connections"), json!({"url": url, "bearer": {"token": "tok-123"}})).await;
    let (_, listed) = call(&app, "GET", &format!("/api/v1/tenants/{mine}/connections"), Value::Null).await;
    let id: Uuid = serde_json::from_value(created["id"].clone()).unwrap();
    let stored: String = sqlx::query_scalar("SELECT access_token FROM connected_accounts WHERE id = $1").bind(id).fetch_one(&pool).await.unwrap();

    assert_eq!(status, StatusCode::OK, "{created}");
    assert_eq!((created["provider"].as_str(), created["account_id"].as_str(), created["url"].as_str()), (Some("url"), Some(id.to_string().as_str()), Some(url.as_str())));
    assert_eq!(created["auth"], json!("bearer"));
    assert_eq!(listed, json!([created]));
    assert!(!listed.to_string().contains("tok-123"), "no token is sent back: {listed}");
    assert_ne!(stored, "tok-123", "the token is ciphertext in the database");
    assert_eq!(connections(&pool).secret(mine, id).await.unwrap(), Secret::Bearer("tok-123".into()));
}

#[sqlx::test(migrations = false)]
async fn an_api_key_is_sent_in_x_api_key_unless_another_header_is_given(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let mine = tenant(&pool).await;
    let app = app(&pool, grants(mine));
    let url = site("<html></html>").await;
    let path = format!("/api/v1/tenants/{mine}/connections");

    let (_, default) = call(&app, "POST", &path, json!({"url": format!("{url}/a"), "api_key": {"key": "k1"}})).await;
    let (_, named) = call(&app, "POST", &path, json!({"url": format!("{url}/b"), "api_key": {"key": "k2", "header": "X-Shop-Key"}})).await;
    let secret = |connection: &Value| serde_json::from_value::<Uuid>(connection["id"].clone()).unwrap();

    assert_eq!((default["auth"].as_str(), default["key_header"].as_str()), (Some("api_key"), Some("X-API-Key")));
    assert_eq!(connections(&pool).secret(mine, secret(&default)).await.unwrap(), Secret::ApiKey { header: "X-API-Key".into(), key: "k1".into() });
    assert_eq!(connections(&pool).secret(mine, secret(&named)).await.unwrap(), Secret::ApiKey { header: "X-Shop-Key".into(), key: "k2".into() });
}

#[sqlx::test(migrations = false)]
async fn the_icon_is_the_sites_link_rel_icon_or_else_its_favicon(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let mine = tenant(&pool).await;
    let app = app(&pool, grants(mine));
    let with_icon = site(r#"<html><head><link rel="shortcut icon" href="/static/logo.png"></head></html>"#).await;
    let without = site("<html><head></head></html>").await;
    let path = format!("/api/v1/tenants/{mine}/connections");

    let (_, linked) = call(&app, "POST", &path, json!({"url": format!("{with_icon}/api/mcp"), "bearer": {"token": "t"}})).await;
    let (_, fallback) = call(&app, "POST", &path, json!({"url": format!("{without}/api/mcp"), "bearer": {"token": "t"}})).await;

    assert_eq!(linked["icon_url"], json!(format!("{with_icon}/static/logo.png")));
    assert_eq!(fallback["icon_url"], json!(format!("{without}/favicon.ico")));
}

#[sqlx::test(migrations = false)]
async fn known_accounts_of_the_tenant_are_listed_and_other_tenants_are_not(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let (mine, other) = (tenant(&pool).await, tenant(&pool).await);
    sqlx::query("INSERT INTO connected_accounts (account_id, provider, email, access_token, tenant_id, auth) VALUES ('g1', 'google', 'shop@example.com', 'x', $1, 'oauth'), ('g2', 'google', 'else@example.com', 'x', $2, 'oauth')")
        .bind(mine)
        .bind(other)
        .execute(&pool)
        .await
        .unwrap();
    let app = app(&pool, grants(mine));

    let (status, listed) = call(&app, "GET", &format!("/api/v1/tenants/{mine}/connections"), Value::Null).await;

    assert_eq!(status, StatusCode::OK, "{listed}");
    let listed = listed.as_array().unwrap();
    assert_eq!(listed.len(), 1, "{listed:?}");
    assert_eq!((listed[0]["provider"].as_str(), listed[0]["email"].as_str(), listed[0]["auth"].as_str()), (Some("google"), Some("shop@example.com"), Some("oauth")));
}

#[sqlx::test(migrations = false)]
async fn two_tenants_connecting_the_same_url_keep_their_own_token(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let (mine, other) = (tenant(&pool).await, tenant(&pool).await);
    let url = site("<html></html>").await;
    let body = |token: &str| json!({"url": url, "bearer": {"token": token}});

    let (_, first) = call(&app(&pool, grants(mine)), "POST", &format!("/api/v1/tenants/{mine}/connections"), body("mine")).await;
    let (_, second) = call(&app(&pool, grants(other)), "POST", &format!("/api/v1/tenants/{other}/connections"), body("theirs")).await;
    let id = |connection: &Value| serde_json::from_value::<Uuid>(connection["id"].clone()).unwrap();

    assert_eq!(connections(&pool).secret(mine, id(&first)).await.unwrap(), Secret::Bearer("mine".into()));
    assert_eq!(connections(&pool).secret(other, id(&second)).await.unwrap(), Secret::Bearer("theirs".into()));
}

#[sqlx::test(migrations = false)]
async fn a_disconnected_card_stays_and_connects_again(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let mine = tenant(&pool).await;
    let app = app(&pool, grants(mine));
    let url = site("<html></html>").await;
    let path = format!("/api/v1/tenants/{mine}/connections");
    let (_, created) = call(&app, "POST", &path, json!({"url": url, "bearer": {"token": "old"}})).await;
    let id: Uuid = serde_json::from_value(created["id"].clone()).unwrap();

    let (disconnected, _) = call(&app, "POST", &format!("{path}/{id}:disconnect"), Value::Null).await;
    let (_, while_disconnected) = call(&app, "GET", &path, Value::Null).await;
    let no_secret = connections(&pool).secret(mine, id).await;
    let (reconnected, again) = call(&app, "PUT", &format!("{path}/{id}"), json!({"bearer": {"token": "new"}})).await;
    let (_, listed) = call(&app, "GET", &path, Value::Null).await;

    assert!(disconnected.is_success(), "{disconnected}");
    assert_eq!((while_disconnected[0]["id"].clone(), while_disconnected[0]["connected"].clone()), (json!(id), json!(false)));
    assert!(no_secret.is_err(), "a disconnected connection is not called");
    assert_eq!(reconnected, StatusCode::OK, "{again}");
    assert_eq!((listed[0]["id"].clone(), listed[0]["connected"].clone()), (json!(id), json!(true)));
    assert_eq!(connections(&pool).secret(mine, id).await.unwrap(), Secret::Bearer("new".into()));
}

#[sqlx::test(migrations = false)]
async fn a_deleted_card_is_gone(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let mine = tenant(&pool).await;
    let app = app(&pool, grants(mine));
    let url = site("<html></html>").await;
    let path = format!("/api/v1/tenants/{mine}/connections");
    let (_, created) = call(&app, "POST", &path, json!({"url": url, "bearer": {"token": "t"}})).await;
    let id: Uuid = serde_json::from_value(created["id"].clone()).unwrap();

    let (deleted, _) = call(&app, "DELETE", &format!("{path}/{id}"), Value::Null).await;
    let (_, listed) = call(&app, "GET", &path, Value::Null).await;

    assert_eq!(deleted, StatusCode::NO_CONTENT);
    assert_eq!(listed, json!([]));
    assert!(connections(&pool).secret(mine, id).await.is_err());
}

#[sqlx::test(migrations = false)]
async fn another_tenants_connection_is_not_found(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let (mine, other) = (tenant(&pool).await, tenant(&pool).await);
    let url = site("<html></html>").await;
    let (_, created) = call(&app(&pool, grants(mine)), "POST", &format!("/api/v1/tenants/{mine}/connections"), json!({"url": url, "bearer": {"token": "t"}})).await;
    let id = created["id"].as_str().unwrap();
    let theirs = app(&pool, grants(other));
    let path = format!("/api/v1/tenants/{other}/connections/{id}");

    let (disconnect, _) = call(&theirs, "POST", &format!("{path}:disconnect"), Value::Null).await;
    let (reconnect, _) = call(&theirs, "PUT", &path, json!({"bearer": {"token": "stolen"}})).await;
    let (delete, _) = call(&theirs, "DELETE", &path, Value::Null).await;

    assert_eq!((disconnect, reconnect, delete), (StatusCode::NOT_FOUND, StatusCode::NOT_FOUND, StatusCode::NOT_FOUND));
    assert_eq!(connections(&pool).secret(mine, id.parse().unwrap()).await.unwrap(), Secret::Bearer("t".into()));
}

#[sqlx::test(migrations = false)]
async fn a_body_without_an_http_url_or_one_credential_is_refused(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let mine = tenant(&pool).await;
    let app = app(&pool, grants(mine));
    let path = format!("/api/v1/tenants/{mine}/connections");
    let bodies = [
        json!({"bearer": {"token": "t"}}),
        json!({"url": "file:///etc/passwd", "bearer": {"token": "t"}}),
        json!({"url": "not a url", "bearer": {"token": "t"}}),
        json!({"url": "https://example.com"}),
        json!({"url": "https://example.com", "bearer": {"token": "t"}, "api_key": {"key": "k"}}),
    ];

    for body in bodies {
        let (status, answer) = call(&app, "POST", &path, body.clone()).await;
        assert!(status.is_client_error(), "{body} answered {status} {answer}");
    }
    let (_, listed) = call(&app, "GET", &path, Value::Null).await;
    assert_eq!(listed, json!([]));
}

#[sqlx::test(migrations = false)]
async fn the_routes_need_the_tenants_grant(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let (mine, other) = (tenant(&pool).await, tenant(&pool).await);
    let app = app(&pool, grants(other));

    let (listing, _) = call(&app, "GET", &format!("/api/v1/tenants/{mine}/connections"), Value::Null).await;
    let (creating, _) = call(&app, "POST", &format!("/api/v1/tenants/{mine}/connections"), json!({"url": "https://example.com", "bearer": {"token": "t"}})).await;

    assert_eq!((listing, creating), (StatusCode::FORBIDDEN, StatusCode::FORBIDDEN));
}

#[sqlx::test(migrations = false)]
async fn an_expired_oauth_token_is_refreshed_before_it_is_used(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let mine = tenant(&pool).await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let token = || async { axum::Json(json!({"access_token": "fresh", "token_type": "bearer", "expires_in": 3600})) };
    tokio::spawn(async move { axum::serve(listener, Router::new().route("/token", axum::routing::post(token))).await.unwrap() });
    let mut google = megh::OAuthProviderConfig::google("client-id", Some("secret".into()));
    google.token_url = format!("{base}/token");
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO connected_accounts (account_id, provider, access_token, refresh_token, expiry, tenant_id, auth) \
         VALUES ('inbox', 'google', 'stale', 'refresh', NOW() - INTERVAL '1 hour', $1, 'oauth') RETURNING id",
    )
    .bind(mine)
    .fetch_one(&pool)
    .await
    .unwrap();
    let connections = Connections::new(pool.clone()).with_providers(std::sync::Arc::new([("google".to_string(), google)].into()));

    assert_eq!(connections.secret(mine, id).await.unwrap(), Secret::Bearer("fresh".into()));
}

#[sqlx::test(migrations = false)]
async fn a_connections_spec_url_is_kept_and_shown(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let mine = tenant(&pool).await;
    let app = app(&pool, grants(mine));
    let url = site("<html></html>").await;
    let path = format!("/api/v1/tenants/{mine}/connections");

    let (_, with_spec) = call(&app, "POST", &path, json!({"url": format!("{url}/v3"), "spec_url": format!("{url}/openapi.json"), "bearer": {"token": "t"}})).await;
    let (_, without) = call(&app, "POST", &path, json!({"url": format!("{url}/v4"), "bearer": {"token": "t"}})).await;
    let id = with_spec["id"].as_str().unwrap();
    call(&app, "POST", &format!("{path}/{id}:disconnect"), Value::Null).await;
    let (_, again) = call(&app, "PUT", &format!("{path}/{id}"), json!({"bearer": {"token": "new"}})).await;

    assert_eq!(with_spec["spec_url"], json!(format!("{url}/openapi.json")));
    assert_eq!(without["spec_url"], Value::Null);
    assert_eq!(again["spec_url"], json!(format!("{url}/openapi.json")), "a reconnect keeps it");
}

#[sqlx::test(migrations = false)]
async fn a_spec_url_that_is_not_http_is_refused(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let mine = tenant(&pool).await;
    let app = app(&pool, grants(mine));
    let path = format!("/api/v1/tenants/{mine}/connections");

    for spec_url in ["", "not a url", "file:///etc/passwd"] {
        let (status, answer) = call(&app, "POST", &path, json!({"url": "https://example.com", "spec_url": spec_url, "bearer": {"token": "t"}})).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{spec_url:?} answered {answer}");
    }
    let (_, listed) = call(&app, "GET", &path, Value::Null).await;
    assert_eq!(listed, json!([]));
}
