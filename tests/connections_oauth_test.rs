//! Specification: connecting a tenant to any site by OAuth (issue #234), with `rmcp`'s OAuth client.
//!
//! - `GET /tenants/{tenant_id}/connections/oauth?url={url}` finds the site's authorization server (RFC 9728, 8414),
//!   registers as a client (RFC 7591) with the callback `{app_origin}{this path}/callback`, and sends the browser to
//!   sign in with PKCE. A site that offers no OAuth is refused (`400`), as is a `spec_url` that is not an http(s) URL.
//! - `GET /tenants/{tenant_id}/connections/oauth/callback?code&state` takes the token and connects the site: a
//!   generic connection with `auth` `oauth`, its URL and icon. The page tells its opener `oauth_connect_success`. A
//!   state this session did not start is refused and connects nothing.
//! - With `&id={id}`, the flow connects the tenant's disconnected card `id` again instead of adding one.
//! - `Connections::secret` answers the token, refreshed at the site when it expires.
//! - As megh reads a path, the routes need the `oauth` and `callback` grants beside `connections`.

use std::sync::{Arc, Mutex};

use axum::body::Body;
use axum::extract::{Form, State};
use axum::http::{header, Request, Response, StatusCode};
use axum::middleware::from_fn;
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use megh::account::{Connections, Secret};
use megh::{authorizer, connections_router, Encryptor, Grant, MeghAuthState};
use serde_json::{json, Value};
use sqlx::PgPool;
use std::collections::HashMap;
use tower::ServiceExt;
use tower_sessions::{MemoryStore, SessionManagerLayer};
use uuid::Uuid;

const KEY: [u8; 32] = [9u8; 32];

/// A site with an OAuth server: what it was registered with, and how long the tokens it gives last.
#[derive(Clone)]
struct Site {
    origin: String,
    registered: Arc<Mutex<Value>>,
    expires_in: u64,
}

async fn protected_resource(State(site): State<Site>) -> Json<Value> {
    Json(json!({"resource": format!("{}/mcp", site.origin), "authorization_servers": [site.origin]}))
}

async fn authorization_server(State(site): State<Site>) -> Json<Value> {
    let origin = &site.origin;
    Json(json!({
        "issuer": origin,
        "authorization_endpoint": format!("{origin}/authorize"),
        "token_endpoint": format!("{origin}/token"),
        "registration_endpoint": format!("{origin}/register"),
        "response_types_supported": ["code"],
        "grant_types_supported": ["authorization_code", "refresh_token"],
        "code_challenge_methods_supported": ["S256"],
        "token_endpoint_auth_methods_supported": ["none"],
    }))
}

async fn register(State(site): State<Site>, Json(body): Json<Value>) -> (StatusCode, Json<Value>) {
    *site.registered.lock().unwrap() = body.clone();
    (StatusCode::CREATED, Json(json!({"client_id": "registered-client", "redirect_uris": body["redirect_uris"]})))
}

async fn token(State(site): State<Site>, Form(form): Form<HashMap<String, String>>) -> Json<Value> {
    let access = match form.get("grant_type").map(String::as_str) {
        Some("refresh_token") => "refreshed",
        _ => "first",
    };
    Json(json!({"access_token": access, "token_type": "Bearer", "expires_in": site.expires_in, "refresh_token": "refresh"}))
}

async fn serve(router: Router) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    origin
}

/// A site offering OAuth whose tokens last `expires_in` seconds.
async fn site(expires_in: u64) -> Site {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let site = Site { origin, registered: Arc::default(), expires_in };
    let router = Router::new()
        .route("/", get(|| async { axum::response::Html(r#"<link rel="icon" href="/logo.svg">"#) }))
        .route("/.well-known/oauth-protected-resource", get(protected_resource))
        .route("/.well-known/oauth-protected-resource/mcp", get(protected_resource))
        .route("/.well-known/oauth-authorization-server", get(authorization_server))
        .route("/register", post(register))
        .route("/token", post(token))
        .with_state(site.clone());
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    site
}

async fn tenant(pool: &PgPool) -> Uuid {
    sqlx::query_scalar("INSERT INTO tenants (name) VALUES ('Shop') RETURNING id").fetch_one(pool).await.unwrap()
}

fn app(pool: &PgPool, tenant: Uuid) -> Router {
    let state = MeghAuthState::new(pool.clone()).with_token_encryptor(Encryptor::new(&KEY)).with_app_origin("http://api.test");
    let grants: Vec<Grant> = ["connections:*", "oauth:*", "callback:*"].iter().map(|grant| grant.to_string()).chain([format!("tenants:read:{tenant}")]).map(Grant::new).collect();
    Router::new()
        .nest("/api/v1", connections_router(state).layer(from_fn(authorizer)))
        .layer(Extension(grants))
        .layer(SessionManagerLayer::new(MemoryStore::default()))
}

async fn get_page(app: &Router, uri: &str, cookie: &str) -> Response<Body> {
    app.clone().oneshot(Request::get(uri).header(header::COOKIE, cookie).body(Body::empty()).unwrap()).await.unwrap()
}

async fn text(response: Response<Body>) -> String {
    String::from_utf8(axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap().to_vec()).unwrap()
}

fn location(response: &Response<Body>) -> reqwest::Url {
    reqwest::Url::parse(response.headers()[header::LOCATION].to_str().unwrap()).unwrap()
}

/// Connects `tenant` to the site by OAuth through `app`, as the site's sign-in page would send the browser back.
async fn connect(app: &Router, tenant: Uuid, site: &Site) -> (Response<Body>, HashMap<String, String>) {
    let start = get_page(app, &format!("/api/v1/tenants/{tenant}/connections/oauth?url={}/mcp", site.origin), "").await;
    assert!(start.status().is_redirection(), "{}: {}", start.status(), text(start).await);
    let cookie = start.headers()[header::SET_COOKIE].to_str().unwrap().split(';').next().unwrap().to_string();
    let sent: HashMap<String, String> = location(&start).query_pairs().map(|(key, value)| (key.into_owned(), value.into_owned())).collect();
    let callback = format!("/api/v1/tenants/{tenant}/connections/oauth/callback?code=abc&state={}", sent["state"]);
    (get_page(app, &callback, &cookie).await, sent)
}

#[sqlx::test(migrations = false)]
async fn a_site_is_found_registered_with_and_connected_by_oauth(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let mine = tenant(&pool).await;
    let site = site(3600).await;
    let app = app(&pool, mine);
    let callback = format!("http://api.test/api/v1/tenants/{mine}/connections/oauth/callback");

    let (done, sent) = connect(&app, mine, &site).await;
    let page = text(done).await;
    let listed = Connections::new(pool.clone()).with_encryptor(Encryptor::new(&KEY)).list(mine).await.unwrap();

    assert_eq!((sent["client_id"].as_str(), sent["redirect_uri"].as_str(), sent["code_challenge_method"].as_str()), ("registered-client", callback.as_str(), "S256"));
    assert_eq!(site.registered.lock().unwrap()["redirect_uris"], json!([callback]));
    assert!(page.contains("oauth_connect_success") && !page.contains("first"), "{page}");
    assert_eq!(listed.len(), 1, "{listed:?}");
    let connection = &listed[0];
    assert_eq!((connection.provider.as_str(), connection.url.as_deref(), connection.connected), ("url", Some(format!("{}/mcp", site.origin).as_str()), true));
    assert_eq!(serde_json::to_value(connection.auth).unwrap(), json!("oauth"));
    assert_eq!(connection.icon_url.as_deref(), Some(format!("{}/logo.svg", site.origin).as_str()));
    let secret = Connections::new(pool.clone()).with_encryptor(Encryptor::new(&KEY)).secret(mine, connection.id).await.unwrap();
    assert_eq!(secret, Secret::Bearer("first".into()));
}

#[sqlx::test(migrations = false)]
async fn an_expiring_token_is_refreshed_at_the_site(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let mine = tenant(&pool).await;
    let site = site(10).await;
    let app = app(&pool, mine);

    let (done, _) = connect(&app, mine, &site).await;
    assert_eq!(done.status(), StatusCode::OK);
    let connections = Connections::new(pool.clone()).with_encryptor(Encryptor::new(&KEY));
    let id = connections.list(mine).await.unwrap()[0].id;

    assert_eq!(connections.secret(mine, id).await.unwrap(), Secret::Bearer("refreshed".into()));
}

#[sqlx::test(migrations = false)]
async fn a_callback_this_session_did_not_start_connects_nothing(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let mine = tenant(&pool).await;
    let site = site(3600).await;
    let app = app(&pool, mine);
    let start = get_page(&app, &format!("/api/v1/tenants/{mine}/connections/oauth?url={}/mcp", site.origin), "").await;
    let cookie = start.headers()[header::SET_COOKIE].to_str().unwrap().split(';').next().unwrap().to_string();

    let forged = get_page(&app, &format!("/api/v1/tenants/{mine}/connections/oauth/callback?code=abc&state=forged"), &cookie).await;
    let listed = Connections::new(pool.clone()).list(mine).await.unwrap();

    assert!(forged.status().is_client_error(), "{}", forged.status());
    assert!(listed.is_empty(), "{listed:?}");
}

#[sqlx::test(migrations = false)]
async fn a_site_that_offers_no_oauth_is_refused(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let mine = tenant(&pool).await;
    let plain = serve(Router::new().route("/", get(|| async { "no oauth here" }))).await;
    let app = app(&pool, mine);

    let refused = get_page(&app, &format!("/api/v1/tenants/{mine}/connections/oauth?url={plain}/api"), "").await;

    assert_eq!(refused.status(), StatusCode::BAD_REQUEST);
}

#[sqlx::test(migrations = false)]
async fn a_disconnected_oauth_card_is_connected_again_not_added(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let mine = tenant(&pool).await;
    let site = site(3600).await;
    let app = app(&pool, mine);
    connect(&app, mine, &site).await;
    let connections = Connections::new(pool.clone()).with_encryptor(Encryptor::new(&KEY));
    let id = connections.list(mine).await.unwrap()[0].id;
    connections.disconnect(mine, id).await.unwrap();

    let start = get_page(&app, &format!("/api/v1/tenants/{mine}/connections/oauth?url={}/mcp&id={id}", site.origin), "").await;
    let cookie = start.headers()[header::SET_COOKIE].to_str().unwrap().split(';').next().unwrap().to_string();
    let state = location(&start).query_pairs().find(|(key, _)| key == "state").unwrap().1.into_owned();
    let done = get_page(&app, &format!("/api/v1/tenants/{mine}/connections/oauth/callback?code=abc&state={state}"), &cookie).await;
    let listed = connections.list(mine).await.unwrap();

    assert_eq!(done.status(), StatusCode::OK);
    assert_eq!(listed.iter().map(|connection| (connection.id, connection.connected)).collect::<Vec<_>>(), [(id, true)]);
    assert_eq!(connections.secret(mine, id).await.unwrap(), Secret::Bearer("first".into()));
}

#[sqlx::test(migrations = false)]
async fn a_site_connected_by_oauth_keeps_its_spec_url(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let mine = tenant(&pool).await;
    let site = site(3600).await;
    let app = app(&pool, mine);
    let spec = format!("{}/openapi.json", site.origin);

    let start = get_page(&app, &format!("/api/v1/tenants/{mine}/connections/oauth?url={}/mcp&spec_url={spec}", site.origin), "").await;
    let cookie = start.headers()[header::SET_COOKIE].to_str().unwrap().split(';').next().unwrap().to_string();
    let state = location(&start).query_pairs().find(|(key, _)| key == "state").unwrap().1.into_owned();
    get_page(&app, &format!("/api/v1/tenants/{mine}/connections/oauth/callback?code=abc&state={state}"), &cookie).await;
    let listed = Connections::new(pool.clone()).list(mine).await.unwrap();

    assert_eq!(listed[0].spec_url.as_deref(), Some(spec.as_str()));
}

#[sqlx::test(migrations = false)]
async fn an_oauth_start_with_a_spec_url_that_is_not_http_is_refused(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let mine = tenant(&pool).await;
    let site = site(3600).await;
    let app = app(&pool, mine);

    let refused = get_page(&app, &format!("/api/v1/tenants/{mine}/connections/oauth?url={}/mcp&spec_url=file:///etc/passwd", site.origin), "").await;

    assert_eq!(refused.status(), StatusCode::BAD_REQUEST);
}
