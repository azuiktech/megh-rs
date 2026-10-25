//! A machine trades its client id and secret for the access token that the API then accepts as a bearer token.

use std::time::Duration;

use axum::body::Body;
use axum::http::{header, Request, Response, StatusCode};
use axum::middleware::{from_fn, from_fn_with_state};
use axum::routing::get;
use axum::Router;
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use megh::auth::client_credentials::token_router;
use megh::auth::http::MeghAuthState;
use megh::auth::oauth::{ClientId, ClientSecret, TokenUrl};
use megh::auth::{jwt_session, oauth_http_client, BasicClient, JwtSession, Registered, Scope, TokenResponse, UserRepo};
use megh::UpsertUserInput;
use serde_json::Value;
use sqlx::PgPool;
use tower::ServiceExt;
use tower_sessions::{MemoryStore, SessionManagerLayer};
use uuid::Uuid;

const SECRET: &[u8] = b"client-credentials-test-secret-32";
const FORM: &str = "application/x-www-form-urlencoded";

fn app(pool: PgPool) -> Router {
    let jwt = JwtSession::new(SECRET, Duration::from_secs(300), pool.clone());
    let api = Router::new()
        .route("/invoices", get(|| async { "ok" }).post(|| async { "ok" }))
        .layer(from_fn(megh::authorizer))
        .layer(from_fn_with_state(jwt.clone(), jwt_session));
    api.merge(token_router(MeghAuthState::new(pool), jwt)).layer(SessionManagerLayer::new(MemoryStore::default()))
}

async fn machine(pool: &PgPool, grants: &[&str]) -> (Registered, Uuid) {
    let org: Uuid = sqlx::query_scalar("INSERT INTO organizations (name) VALUES ('Acme') RETURNING id").fetch_one(pool).await.unwrap();
    let grants = grants.iter().map(|grant| grant.to_string()).collect();
    (UserRepo::new(pool).register_machine(org, "ci", grants).await.unwrap(), org)
}

async fn exchange(app: &Router, credentials: Option<(&str, &str)>, form: &str) -> Response<Body> {
    let request = Request::post("/auth/megh/token").header(header::CONTENT_TYPE, FORM);
    let request = match credentials {
        Some((id, secret)) => request.header(header::AUTHORIZATION, format!("Basic {}", STANDARD.encode(format!("{id}:{secret}")))),
        None => request,
    };
    app.clone().oneshot(request.body(Body::from(form.to_string())).unwrap()).await.unwrap()
}

async fn json(response: Response<Body>) -> Value {
    serde_json::from_slice(&axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap()
}

async fn call(app: &Router, method: &str, token: &str) -> StatusCode {
    let request = Request::builder().method(method).uri("/invoices").header(header::AUTHORIZATION, format!("Bearer {token}"));
    app.clone().oneshot(request.body(Body::empty()).unwrap()).await.unwrap().status()
}

#[sqlx::test]
async fn registering_makes_a_machine_user_with_one_membership_and_a_hashed_secret(pool: PgPool) {
    let (registered, org) = machine(&pool, &["invoices:read"]).await;

    let (provider, account_id, email, hash): (String, String, String, String) =
        sqlx::query_as("SELECT provider, account_id, email, password_hash FROM users").fetch_one(&pool).await.unwrap();
    assert_eq!((provider.as_str(), account_id.as_str(), email.as_str()), ("megh", registered.client_id.as_str(), registered.client_id.as_str()));
    assert!(hash.starts_with("$2") && !hash.contains(&registered.secret));
    let (member_org, role, grants): (Uuid, String, String) =
        sqlx::query_as("SELECT organization_id, role, grants FROM organization_members").fetch_one(&pool).await.unwrap();
    assert_eq!((member_org, role.as_str(), grants.as_str()), (org, "machine", r#"["invoices:read"]"#));
}

#[sqlx::test]
async fn the_secret_is_traded_for_an_access_token_the_api_accepts_as_a_bearer_token(pool: PgPool) {
    let (registered, _) = machine(&pool, &["invoices:read"]).await;
    let app = app(pool);

    let response = exchange(&app, Some((&registered.client_id, &registered.secret)), "grant_type=client_credentials").await;

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    let body = json(response).await;
    assert!(body["token_type"].as_str().unwrap().eq_ignore_ascii_case("bearer"));
    assert_eq!((body["expires_in"].as_u64(), body["scope"].as_str()), (Some(300), Some("invoices:read")));
    assert!(body.get("refresh_token").is_none());
    let token = body["access_token"].as_str().unwrap();
    assert_eq!(call(&app, "GET", token).await, StatusCode::OK);
    assert_eq!(call(&app, "POST", token).await, StatusCode::FORBIDDEN, "the grants in the token decide");
}

#[sqlx::test]
async fn a_requested_scope_only_narrows_the_grants(pool: PgPool) {
    let (registered, _) = machine(&pool, &["invoices:*"]).await;
    let app = app(pool);
    let credentials = Some((registered.client_id.as_str(), registered.secret.as_str()));

    let narrowed = json(exchange(&app, credentials, "grant_type=client_credentials&scope=invoices%3Aread").await).await;
    let widened = exchange(&app, credentials, "grant_type=client_credentials&scope=payments%3Aread").await;

    assert_eq!(narrowed["scope"], "invoices:read");
    let token = narrowed["access_token"].as_str().unwrap();
    assert_eq!((call(&app, "GET", token).await, call(&app, "POST", token).await), (StatusCode::OK, StatusCode::FORBIDDEN));
    assert_eq!(widened.status(), StatusCode::BAD_REQUEST);
    assert_eq!(json(widened).await["error"], "invalid_scope");
}

#[sqlx::test]
async fn bad_client_credentials_are_invalid_client(pool: PgPool) {
    let (registered, _) = machine(&pool, &["invoices:read"]).await;
    let app = app(pool.clone());
    let human = UserRepo::new(&pool)
        .upsert(&UpsertUserInput { provider: "google".into(), account_id: "1".into(), email: "ann@example.com".into(), display_name: None, photo_url: None })
        .await
        .unwrap();
    UserRepo::new(&pool).set_password(human.id, "s3cret-pass").await.unwrap();
    let form = "grant_type=client_credentials";

    let attempts = [
        exchange(&app, Some((&registered.client_id, "wrong")), form).await,
        exchange(&app, Some(("nobody", &registered.secret)), form).await,
        exchange(&app, Some(("ann@example.com", "s3cret-pass")), form).await,
        exchange(&app, None, form).await,
    ];

    for response in attempts {
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(response.headers()[header::WWW_AUTHENTICATE], "Basic");
        assert_eq!(json(response).await["error"], "invalid_client");
    }
}

#[sqlx::test]
async fn a_deleted_machine_gets_no_token(pool: PgPool) {
    let (registered, _) = machine(&pool, &["invoices:read"]).await;
    let app = app(pool.clone());
    sqlx::query("UPDATE users SET deleted_at = NOW()").execute(&pool).await.unwrap();

    let response = exchange(&app, Some((&registered.client_id, &registered.secret)), "grant_type=client_credentials").await;

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[sqlx::test]
async fn other_grant_types_and_malformed_requests_are_refused(pool: PgPool) {
    let (registered, _) = machine(&pool, &["invoices:read"]).await;
    let app = app(pool);
    let credentials = Some((registered.client_id.as_str(), registered.secret.as_str()));

    let password = exchange(&app, credentials, "grant_type=password&username=a&password=b").await;
    let missing = exchange(&app, credentials, "scope=invoices%3Aread").await;

    assert_eq!((password.status(), json(password).await["error"].clone()), (StatusCode::BAD_REQUEST, "unsupported_grant_type".into()));
    assert_eq!((missing.status(), json(missing).await["error"].clone()), (StatusCode::BAD_REQUEST, "invalid_request".into()));
}

#[sqlx::test]
async fn the_oauth2_client_library_gets_a_token_from_the_endpoint(pool: PgPool) {
    let (registered, _) = machine(&pool, &["invoices:read"]).await;
    let app = app(pool);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn({
        let app = app.clone();
        async move { axum::serve(listener, app).await.unwrap() }
    });
    let client = BasicClient::new(ClientId::new(registered.client_id))
        .set_client_secret(ClientSecret::new(registered.secret))
        .set_token_uri(TokenUrl::new(format!("{base}/auth/megh/token")).unwrap());

    let response = client
        .exchange_client_credentials()
        .add_scope(Scope::new("invoices:read".into()))
        .request_async(&oauth_http_client(reqwest::Client::new()))
        .await
        .unwrap();

    assert_eq!(response.expires_in(), Some(Duration::from_secs(300)));
    assert_eq!(call(&app, "GET", response.access_token().secret()).await, StatusCode::OK);
}
