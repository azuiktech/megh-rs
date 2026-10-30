//! A tenant's connections: its accounts at known providers and at generic sites, connected by URL with a bearer token,
//! an API key or OAuth. Each shows as a card that connects, disconnects and is deleted.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use reqwest::Url;
use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use oauth2::TokenResponse;
use rmcp::transport::auth::StoredCredentials;

use super::accounts::Accounts;
use super::site_oauth;
use super::encryption::Encryptor;
use crate::auth::OAuthProviderConfig;

/// How a connection signs its requests.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[serde(rename_all = "snake_case")]
#[sqlx(type_name = "text", rename_all = "snake_case")]
pub enum Auth {
    #[serde(rename = "oauth")]
    #[sqlx(rename = "oauth")]
    OAuth,
    Bearer,
    ApiKey,
}

/// A tenant's account at a provider or a site, as its card shows it: never its token.
#[derive(Debug, Clone, PartialEq, Serialize, sqlx::FromRow)]
pub struct Connection {
    pub id: Uuid,
    pub provider: String,
    pub account_id: String,
    pub email: Option<String>,
    pub url: Option<String>,
    /// The OpenAPI document of the REST API at `url`.
    pub spec_url: Option<String>,
    pub icon_url: Option<String>,
    pub auth: Option<Auth>,
    pub key_header: Option<String>,
    pub connected: bool,
    pub created_at: Option<DateTime<Utc>>,
}

/// What a request to the connection carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Secret {
    Bearer(String),
    ApiKey { header: String, key: String },
}

/// Why a connection cannot be used.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ConnectionError {
    #[error("connection not found")]
    NotFound,
    #[error("connection is disconnected")]
    Disconnected,
    #[error("{0}")]
    Invalid(&'static str),
    #[error("token encryption failed")]
    Encryption,
    #[error(transparent)]
    Account(#[from] super::accounts::AccountError),
    #[error("the site's OAuth failed: {0}")]
    OAuth(String),
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}

/// How a generic connection signs in: exactly one of a bearer token and an API key.
#[derive(Debug, Deserialize)]
pub struct Credentials {
    bearer: Option<Bearer>,
    api_key: Option<ApiKey>,
}

#[derive(Debug, Deserialize)]
struct Bearer {
    token: String,
}

#[derive(Debug, Deserialize)]
struct ApiKey {
    key: String,
    header: Option<String>,
}

/// What `secret` reads of a connection.
#[derive(sqlx::FromRow)]
struct SecretRow {
    provider: String,
    account_id: String,
    url: Option<String>,
    access_token: String,
    auth: Option<Auth>,
    key_header: Option<String>,
    connected: bool,
}

/// A generic connection to make: the site's URL and how it signs in.
#[derive(Debug, Deserialize)]
pub struct NewConnection {
    url: String,
    spec_url: Option<String>,
    #[serde(flatten)]
    credentials: Credentials,
}

const DEFAULT_KEY_HEADER: &str = "X-API-Key";
/// The provider of a generic connection: a site, by its URL.
const SITE: &str = "url";
const COLUMNS: &str = "id, provider, account_id, email, url, spec_url, icon_url, auth, key_header, disconnected_at IS NULL AS connected, created_at";
const OF_TENANT: &str = "tenant_id = $1 AND id = $2 AND deleted_at IS NULL";

fn http_url(url: &str, invalid: &'static str) -> Result<Url, ConnectionError> {
    Url::parse(url).ok().filter(|url| matches!(url.scheme(), "http" | "https")).ok_or(ConnectionError::Invalid(invalid))
}

fn check_spec_url(spec_url: Option<&str>) -> Result<(), ConnectionError> {
    spec_url.map_or(Ok(()), |spec| http_url(spec, "spec_url must be an http(s) URL").map(drop))
}

impl Credentials {
    /// The token stored, how it is sent, and the header an API key goes in.
    fn stored(self) -> Result<(String, Auth, Option<String>), ConnectionError> {
        match (self.bearer, self.api_key) {
            (Some(bearer), None) => Ok((bearer.token, Auth::Bearer, None)),
            (None, Some(key)) => Ok((key.key, Auth::ApiKey, Some(key.header.unwrap_or_else(|| DEFAULT_KEY_HEADER.into())))),
            _ => Err(ConnectionError::Invalid("give exactly one of bearer and api_key")),
        }
    }
}

/// The tenants' connections. Holds the injected pool, the encryptor of stored tokens, the known providers whose
/// tokens are refreshed, and the client icons are read with.
#[derive(Clone)]
pub struct Connections {
    pool: PgPool,
    encryptor: Option<Encryptor>,
    providers: Arc<HashMap<String, OAuthProviderConfig>>,
    http: reqwest::Client,
}

impl Connections {
    pub fn new(pool: PgPool) -> Self {
        let http = reqwest::Client::builder().timeout(Duration::from_secs(10)).build().expect("static HTTP client configuration");
        Self { pool, encryptor: None, providers: Arc::default(), http }
    }

    /// Encrypts tokens before every write and decrypts them after every read.
    pub fn with_encryptor(mut self, encryptor: Encryptor) -> Self {
        self.encryptor = Some(encryptor);
        self
    }

    /// The tenant's connections, connected or not; deleted ones are gone.
    pub async fn list(&self, tenant_id: Uuid) -> Result<Vec<Connection>, ConnectionError> {
        let sql = format!("SELECT {COLUMNS} FROM connected_accounts WHERE tenant_id = $1 AND deleted_at IS NULL ORDER BY created_at, id");
        Ok(sqlx::query_as(&sql).bind(tenant_id).fetch_all(&self.pool).await?)
    }

    /// The known providers whose expired OAuth tokens are refreshed.
    pub fn with_providers(mut self, providers: Arc<HashMap<String, OAuthProviderConfig>>) -> Self {
        self.providers = providers;
        self
    }

    /// What a request to the tenant's connected connection `id` carries.
    pub async fn secret(&self, tenant_id: Uuid, id: Uuid) -> Result<Secret, ConnectionError> {
        let sql = format!("SELECT provider, account_id, url, access_token, auth, key_header, disconnected_at IS NULL AS connected FROM connected_accounts WHERE {OF_TENANT}");
        let row: Option<SecretRow> = sqlx::query_as(&sql).bind(tenant_id).bind(id).fetch_optional(&self.pool).await?;
        let row = row.ok_or(ConnectionError::NotFound)?;
        if !row.connected {
            return Err(ConnectionError::Disconnected);
        }
        Ok(match (row.auth, row.url) {
            (Some(Auth::ApiKey), _) => Secret::ApiKey { header: row.key_header.unwrap_or_else(|| DEFAULT_KEY_HEADER.into()), key: self.decrypt(&row.access_token)? },
            (Some(Auth::OAuth), Some(url)) if row.provider == SITE => Secret::Bearer(site_oauth::access_token(&url, self.grant(id)).await?),
            (Some(Auth::OAuth), _) => Secret::Bearer(self.accounts().access_token(&row.provider, &row.account_id).await?),
            (Some(Auth::Bearer) | None, _) => Secret::Bearer(self.decrypt(&row.access_token)?),
        })
    }

    /// Connects the tenant to the site at `url`, with the site's icon.
    pub async fn create(&self, tenant_id: Uuid, new: NewConnection) -> Result<Connection, ConnectionError> {
        let url = http_url(&new.url, "url must be an http(s) URL")?;
        check_spec_url(new.spec_url.as_deref())?;
        let (token, auth, header) = new.credentials.stored()?;
        let icon = self.icon_of(&url).await;
        let id = Uuid::now_v7();
        let sql = format!(
            "INSERT INTO connected_accounts (id, account_id, provider, access_token, tenant_id, url, icon_url, auth, key_header, spec_url) \
             VALUES ($2, $3, 'url', $4, $1, $5, $6, $7, $8, $9) RETURNING {COLUMNS}"
        );
        let query = sqlx::query_as(&sql).bind(tenant_id).bind(id).bind(id.to_string()).bind(self.encrypt(&token)?);
        Ok(query.bind(&new.url).bind(icon.as_str()).bind(auth).bind(header).bind(&new.spec_url).fetch_one(&self.pool).await?)
    }

    /// Connects the tenant to the site at `url` with the OAuth credentials rmcp got from it.
    pub(crate) async fn create_oauth(&self, tenant_id: Uuid, (url, spec_url): (&str, Option<&str>), credentials: StoredCredentials) -> Result<Connection, ConnectionError> {
        let parsed = Url::parse(url).map_err(|_| ConnectionError::Invalid("url must be an http(s) URL"))?;
        let icon = self.icon_of(&parsed).await;
        let access = credentials.token_response.as_ref().map(|token| token.access_token().secret().clone()).unwrap_or_default();
        let grant = serde_json::to_string(&credentials).map_err(|_| ConnectionError::Encryption)?;
        let id = Uuid::now_v7();
        let sql = format!(
            "INSERT INTO connected_accounts (id, account_id, provider, access_token, tenant_id, url, icon_url, auth, oauth, spec_url) \
             VALUES ($2, $3, 'url', $4, $1, $5, $6, 'oauth', $7, $8) RETURNING {COLUMNS}"
        );
        let query = sqlx::query_as(&sql).bind(tenant_id).bind(id).bind(id.to_string()).bind(self.encrypt(&access)?);
        Ok(query.bind(url).bind(icon.as_str()).bind(self.encrypt(&grant)?).bind(spec_url).fetch_one(&self.pool).await?)
    }

    /// Connects the tenant's site connection `id` again with the OAuth credentials rmcp got from it.
    pub(crate) async fn reconnect_oauth(&self, tenant_id: Uuid, id: Uuid, credentials: StoredCredentials) -> Result<Connection, ConnectionError> {
        let access = credentials.token_response.as_ref().map(|token| token.access_token().secret().clone()).unwrap_or_default();
        let grant = serde_json::to_string(&credentials).map_err(|_| ConnectionError::Encryption)?;
        let sql = format!(
            "UPDATE connected_accounts SET access_token = $3, oauth = $4, auth = 'oauth', key_header = NULL, disconnected_at = NULL, updated_at = NOW() \
             WHERE {OF_TENANT} AND provider = '{SITE}' RETURNING {COLUMNS}"
        );
        let query = sqlx::query_as(&sql).bind(tenant_id).bind(id).bind(self.encrypt(&access)?).bind(self.encrypt(&grant)?);
        query.fetch_optional(&self.pool).await?.ok_or(ConnectionError::NotFound)
    }

    /// Connects the tenant's generic connection `id` again, with new credentials.
    pub async fn reconnect(&self, tenant_id: Uuid, id: Uuid, credentials: Credentials) -> Result<Connection, ConnectionError> {
        let (token, auth, header) = credentials.stored()?;
        let sql = format!(
            "UPDATE connected_accounts SET access_token = $3, auth = $4, key_header = $5, disconnected_at = NULL, updated_at = NOW() \
             WHERE {OF_TENANT} AND provider = 'url' RETURNING {COLUMNS}"
        );
        let query = sqlx::query_as(&sql).bind(tenant_id).bind(id).bind(self.encrypt(&token)?).bind(auth).bind(header);
        query.fetch_optional(&self.pool).await?.ok_or(ConnectionError::NotFound)
    }

    /// Disconnects the tenant's connection `id`; its card stays.
    pub async fn disconnect(&self, tenant_id: Uuid, id: Uuid) -> Result<Connection, ConnectionError> {
        let sql = format!("UPDATE connected_accounts SET disconnected_at = NOW(), updated_at = NOW() WHERE {OF_TENANT} RETURNING {COLUMNS}");
        sqlx::query_as(&sql).bind(tenant_id).bind(id).fetch_optional(&self.pool).await?.ok_or(ConnectionError::NotFound)
    }

    /// Disconnects and deletes the tenant's connection `id`, card and all.
    pub async fn delete(&self, tenant_id: Uuid, id: Uuid) -> Result<(), ConnectionError> {
        let sql = format!("UPDATE connected_accounts SET disconnected_at = COALESCE(disconnected_at, NOW()), deleted_at = NOW(), updated_at = NOW() WHERE {OF_TENANT}");
        let deleted = sqlx::query(&sql).bind(tenant_id).bind(id).execute(&self.pool).await?;
        (deleted.rows_affected() > 0).then_some(()).ok_or(ConnectionError::NotFound)
    }

    /// The icon of the site `url` is on: its home page's `<link rel="icon">`, else its `/favicon.ico`.
    async fn icon_of(&self, url: &Url) -> Url {
        let home = url.join("/").expect("an http(s) URL has a root");
        let favicon = home.join("/favicon.ico").expect("an http(s) URL has a root");
        let page = match self.http.get(home.clone()).send().await {
            Ok(response) => response.text().await.unwrap_or_default(),
            Err(_) => return favicon,
        };
        linked_icon(&page).and_then(|href| home.join(&href).ok()).unwrap_or(favicon)
    }

    /// The OAuth credentials of the site connection `id`, which rmcp loads and saves.
    fn grant(&self, id: Uuid) -> site_oauth::StoredGrant {
        site_oauth::StoredGrant { pool: self.pool.clone(), encryptor: self.encryptor.clone(), id }
    }

    /// The accounts of the known providers, refreshed as they expire.
    fn accounts(&self) -> Accounts {
        let accounts = Accounts::new(self.pool.clone(), self.providers.clone());
        match self.encryptor.clone() {
            Some(encryptor) => accounts.with_encryptor(encryptor),
            None => accounts,
        }
    }

    fn encrypt(&self, plaintext: &str) -> Result<String, ConnectionError> {
        match &self.encryptor {
            Some(encryptor) => encryptor.encrypt(plaintext).map_err(|_| ConnectionError::Encryption),
            None => Ok(plaintext.to_string()),
        }
    }

    fn decrypt(&self, stored: &str) -> Result<String, ConnectionError> {
        match &self.encryptor {
            Some(encryptor) => encryptor.decrypt(stored).map_err(|_| ConnectionError::Encryption),
            None => Ok(stored.to_string()),
        }
    }
}

/// The `href` of the page's first `<link>` whose `rel` includes `icon`.
fn linked_icon(page: &str) -> Option<String> {
    let links = Selector::parse("link[rel~=icon][href]").expect("a static selector");
    Html::parse_document(page).select(&links).next().and_then(|link| link.attr("href")).map(str::to_string)
}

#[cfg(feature = "axum")]
pub use routes::connections_router;

#[cfg(feature = "axum")]
mod routes {
    use axum::extract::{FromRef, OriginalUri, Path, Query, State};
    use axum::http::StatusCode;
    use axum::response::{IntoResponse, Redirect, Response};
    use axum::routing::{get, post, put};
    use axum::{Json, Router};
    use rmcp::transport::auth::{AuthError, StateStore, StoredAuthorizationState};
    use serde::{Deserialize, Serialize};
    use tower_sessions::Session;
    use uuid::Uuid;

    use super::{site_oauth, Connection, ConnectionError, Connections, Credentials, NewConnection};
    use crate::auth::http::popup;
    use crate::auth::MeghAuthState;

    #[derive(Clone)]
    struct Shared {
        connections: Connections,
        auth: MeghAuthState,
    }

    impl FromRef<Shared> for Connections {
        fn from_ref(shared: &Shared) -> Self {
            shared.connections.clone()
        }
    }

    impl FromRef<Shared> for MeghAuthState {
        fn from_ref(shared: &Shared) -> Self {
            shared.auth.clone()
        }
    }

    /// `GET`/`POST /tenants/{tenant_id}/connections`, `PUT`/`DELETE /tenants/{tenant_id}/connections/{id}`,
    /// `POST /tenants/{tenant_id}/connections/{id}:disconnect`, and the OAuth flow with a site,
    /// `GET /tenants/{tenant_id}/connections/oauth?url=` (`&spec_url=` its OpenAPI document, `&id=` to connect that card again) and its `/callback`; to be nested under the API behind
    /// `authorizer`, with a session layer. The `:disconnect` route needs `megh::booking::rewrite_aip_136` around the app.
    pub fn connections_router(state: MeghAuthState) -> Router {
        let connections = Connections::new(state.pool.clone()).with_providers(state.providers.clone());
        let connections = match state.token_encryptor.clone() {
            Some(encryptor) => connections.with_encryptor(encryptor),
            None => connections,
        };
        Router::new()
            .route("/tenants/{tenant_id}/connections", get(list).post(create))
            .route("/tenants/{tenant_id}/connections/oauth", get(start_oauth))
            .route("/tenants/{tenant_id}/connections/oauth/callback", get(finish_oauth))
            .route("/tenants/{tenant_id}/connections/{id}", put(reconnect).delete(delete))
            .route("/tenants/{tenant_id}/connections/{id}/{verb}", post(act))
            .with_state(Shared { connections, auth: state })
    }

    async fn list(State(connections): State<Connections>, Path(tenant_id): Path<Uuid>) -> Result<Json<Vec<Connection>>, ConnectionError> {
        Ok(Json(connections.list(tenant_id).await?))
    }

    async fn create(State(connections): State<Connections>, Path(tenant_id): Path<Uuid>, Json(new): Json<NewConnection>) -> Result<Json<Connection>, ConnectionError> {
        Ok(Json(connections.create(tenant_id, new).await?))
    }

    async fn reconnect(
        State(connections): State<Connections>,
        Path((tenant_id, id)): Path<(Uuid, Uuid)>,
        Json(credentials): Json<Credentials>,
    ) -> Result<Json<Connection>, ConnectionError> {
        Ok(Json(connections.reconnect(tenant_id, id, credentials).await?))
    }

    async fn delete(State(connections): State<Connections>, Path((tenant_id, id)): Path<(Uuid, Uuid)>) -> Result<StatusCode, ConnectionError> {
        connections.delete(tenant_id, id).await.map(|()| StatusCode::NO_CONTENT)
    }

    /// `:disconnect`, the one custom method.
    async fn act(State(connections): State<Connections>, Path((tenant_id, id, verb)): Path<(Uuid, Uuid, String)>) -> Result<Json<Connection>, ConnectionError> {
        match verb.as_str() {
            ":disconnect" => Ok(Json(connections.disconnect(tenant_id, id).await?)),
            _ => Err(ConnectionError::NotFound),
        }
    }

    /// The site to sign in at, and the card it connects again, if any.
    #[derive(Deserialize)]
    struct Site {
        url: String,
        spec_url: Option<String>,
        id: Option<Uuid>,
    }

    #[derive(Deserialize)]
    struct Callback {
        code: String,
        state: String,
    }

    /// The connect a sign-in at a site belongs to, kept in the session until its callback.
    #[derive(Serialize, Deserialize)]
    struct Pending {
        url: String,
        spec_url: Option<String>,
        client_id: String,
        id: Option<Uuid>,
    }

    /// Sends the browser to sign in at the site, after finding its OAuth server and registering with it.
    async fn start_oauth(State(auth): State<MeghAuthState>, OriginalUri(uri): OriginalUri, Query(site): Query<Site>, session: Session) -> Result<Redirect, ConnectionError> {
        super::check_spec_url(site.spec_url.as_deref())?;
        let callback = format!("{}{}/callback", auth.app_origin.trim_end_matches('/'), uri.path());
        let started = site_oauth::start(&site.url, &callback, SessionStates(session.clone())).await?;
        let csrf = reqwest::Url::parse(&started.authorize_url).ok().and_then(|url| url.query_pairs().find(|(key, _)| key == "state").map(|(_, state)| state.into_owned()));
        let csrf = csrf.ok_or(ConnectionError::OAuth("the sign-in URL has no state".into()))?;
        session.insert(&pending_key(&csrf), Pending { url: site.url, spec_url: site.spec_url, client_id: started.client_id, id: site.id }).await.map_err(session_failed)?;
        Ok(Redirect::to(&started.authorize_url))
    }

    /// Takes the site's token and connects it; the page tells its opener, as a known provider's connect does.
    async fn finish_oauth(
        State(shared): State<Shared>,
        Path(tenant_id): Path<Uuid>,
        OriginalUri(uri): OriginalUri,
        Query(callback): Query<Callback>,
        session: Session,
    ) -> Result<Response, ConnectionError> {
        let pending: Option<Pending> = session.remove(&pending_key(&callback.state)).await.map_err(session_failed)?;
        let pending = pending.ok_or(ConnectionError::Invalid("no connect in progress"))?;
        let redirect_uri = format!("{}{}", shared.auth.app_origin.trim_end_matches('/'), uri.path());
        let code = (callback.code.as_str(), callback.state.as_str());
        let credentials = site_oauth::finish(&pending.url, &pending.client_id, &redirect_uri, code, SessionStates(session)).await?;
        let connection = match pending.id {
            Some(id) => shared.connections.reconnect_oauth(tenant_id, id, credentials).await?,
            None => shared.connections.create_oauth(tenant_id, (&pending.url, pending.spec_url.as_deref()), credentials).await?,
        };
        Ok(popup(&shared.auth, serde_json::json!({ "type": "oauth_connect_success", "connection": connection }), None))
    }

    fn pending_key(csrf: &str) -> String {
        format!("connections:oauth:{csrf}")
    }

    fn session_failed(error: tower_sessions::session::Error) -> ConnectionError {
        ConnectionError::OAuth(error.to_string())
    }

    /// rmcp's PKCE state of a sign-in, kept in the browser's session.
    struct SessionStates(Session);

    fn state_key(csrf: &str) -> String {
        format!("connections:oauth:state:{csrf}")
    }

    fn state_failed(error: tower_sessions::session::Error) -> AuthError {
        AuthError::InternalError(error.to_string())
    }

    #[async_trait::async_trait]
    impl StateStore for SessionStates {
        async fn save(&self, csrf: &str, state: StoredAuthorizationState) -> Result<(), AuthError> {
            self.0.insert(&state_key(csrf), state).await.map_err(state_failed)
        }

        async fn load(&self, csrf: &str) -> Result<Option<StoredAuthorizationState>, AuthError> {
            self.0.get(&state_key(csrf)).await.map_err(state_failed)
        }

        async fn delete(&self, csrf: &str) -> Result<(), AuthError> {
            self.0.remove::<StoredAuthorizationState>(&state_key(csrf)).await.map(|_| ()).map_err(state_failed)
        }
    }

    impl IntoResponse for ConnectionError {
        fn into_response(self) -> Response {
            let status = match &self {
                Self::NotFound => StatusCode::NOT_FOUND,
                Self::Invalid(_) => StatusCode::BAD_REQUEST,
                Self::Disconnected => StatusCode::CONFLICT,
                Self::OAuth(_) => StatusCode::BAD_GATEWAY,
                Self::Encryption | Self::Account(_) | Self::Database(_) => StatusCode::INTERNAL_SERVER_ERROR,
            };
            let reason = match status {
                StatusCode::INTERNAL_SERVER_ERROR => "internal error".to_string(),
                _ => self.to_string(),
            };
            (status, Json(serde_json::json!({ "error": reason }))).into_response()
        }
    }
}
