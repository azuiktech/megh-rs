//! Ready-to-mount Axum router and AuthUser extractor for Megh Gateway. Sessions come from `tower-sessions`:
//! mount the router under a `SessionManagerLayer`.

use std::collections::HashMap;
use std::sync::Arc;

pub use axum::extract::FromRef;
use axum::{
    extract::{FromRequestParts, Path, Query, State},
    http::{request::Parts, StatusCode},
    response::{Html, IntoResponse, Redirect, Response},
    routing::{get, post},
    Json, Router,
};
use axum_extra::extract::cookie::{Cookie, CookieJar};
use axum_extra::extract::Query as ListQuery;
use axum_extra::headers::{authorization::Basic, Authorization};
use axum_extra::TypedHeader;
use oauth2::{AccessToken, AuthorizationCode, TokenResponse};
use serde::{Deserialize, Serialize};
use tower_http::csrf::CsrfLayer;
use tower_sessions::Session;
use uuid::Uuid;

use crate::account::{ConnectedAccount, ConnectedAccountRepo, Encryptor, OAuth2Tokens};
use crate::auth::oauth::{
    fetch_user_info, oauth_http_client, BasicTokenResponse, CsrfToken, OAuthProviderConfig, OAuthUserInfo, PkceCodeChallenge,
    PkceCodeVerifier, ProviderClient, RedirectUrl, Scope,
};
use crate::auth::token::JWT_COOKIE;
use crate::auth::user::{PasswordError, UpsertUserInput, User, UserRepo};
use crate::tenant::Tenants;

/// Shared state required by the Megh authentication HTTP router.
#[derive(Clone)]
pub struct MeghAuthState {
    pub pool: sqlx::PgPool,
    pub redirect_after_login: String,
    pub app_origin: String,
    pub providers: Arc<HashMap<String, OAuthProviderConfig>>,
    pub http_client: reqwest::Client,
    /// Where the sign-in popup posts its result (the origin of the page that opened it); defaults to `app_origin`.
    pub web_origin: Option<String>,
    /// Encrypts stored OAuth tokens at rest when set (default: plaintext, as before).
    pub token_encryptor: Option<Encryptor>,
}

impl MeghAuthState {
    pub fn new(pool: sqlx::PgPool) -> Self {
        Self {
            pool,
            redirect_after_login: "/".to_string(),
            app_origin: "http://localhost:8080".to_string(),
            providers: Arc::new(HashMap::new()),
            http_client: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(std::time::Duration::from_secs(10))
                .build()
                .expect("static HTTP client configuration"),
            web_origin: None,
            token_encryptor: None,
        }
    }

    pub fn with_token_encryptor(mut self, encryptor: Encryptor) -> Self {
        self.token_encryptor = Some(encryptor);
        self
    }

    pub fn with_redirect_after_login(mut self, redirect: impl Into<String>) -> Self {
        self.redirect_after_login = redirect.into();
        self
    }

    pub fn with_app_origin(mut self, app_origin: impl Into<String>) -> Self {
        self.app_origin = app_origin.into();
        self
    }

    pub fn with_web_origin(mut self, web_origin: impl Into<String>) -> Self {
        self.web_origin = Some(web_origin.into());
        self
    }

    pub fn add_provider(mut self, provider: OAuthProviderConfig) -> Self {
        Arc::make_mut(&mut self.providers).insert(provider.provider_id.clone(), provider);
        self
    }

    fn provider(&self, id: &str) -> Result<&OAuthProviderConfig, AuthError> {
        self.providers.get(id).ok_or(AuthError(StatusCode::NOT_FOUND, "provider not configured"))
    }

    /// The `oauth2` client of `provider`; its callback is `/auth/{provider}/token` on the API origin unless configured.
    fn client(&self, provider: &OAuthProviderConfig) -> Result<ProviderClient, AuthError> {
        let default = format!("{}/auth/{}/token", self.app_origin.trim_end_matches('/'), provider.provider_id);
        let redirect = RedirectUrl::new(provider.redirect_url.clone().unwrap_or(default))?;
        Ok(provider.build_client(Some(redirect))?)
    }

    fn accounts(&self) -> ConnectedAccountRepo<'_> {
        let accounts = ConnectedAccountRepo::new(&self.pool);
        match self.token_encryptor.clone() {
            Some(encryptor) => accounts.with_encryptor(encryptor),
            None => accounts,
        }
    }

    /// Whether `page` is on the web or API origin, the only places a flow may send the browser back to.
    fn is_own_page(&self, page: &str) -> bool {
        let Ok(page) = oauth2::url::Url::parse(page) else { return false };
        let origin = page.origin().ascii_serialization();
        [self.web_origin.as_deref(), Some(self.app_origin.as_str())].into_iter().flatten().any(|own| own.trim_end_matches('/') == origin)
    }

    /// Revokes `token` at the provider (RFC 7009).
    async fn revoke(&self, provider_id: &str, token: &str) -> Result<(), AuthError> {
        let client = self.client(self.provider(provider_id)?)?;
        let request = client.revoke_token(AccessToken::new(token.to_string()).into()).map_err(bad_gateway)?;
        request.request_async(&oauth_http_client(self.http_client.clone())).await.map_err(bad_gateway)
    }
}

/// Response returned by `/auth/me`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthMeResponse {
    pub user: User,
    /// The user's memberships, earliest first.
    pub memberships: Vec<crate::tenant::Member>,
}

pub(crate) const USER_ID: &str = "user_id";

/// Query parameters passed in OAuth callback redirects.
#[derive(Debug, Deserialize)]
pub struct OAuthCallbackQuery {
    pub code: Option<String>,
    pub state: Option<String>,
    pub error: Option<String>,
    pub error_description: Option<String>,
}

/// Axum extractor that injects the authenticated User into route handlers.
#[derive(Debug, Clone)]
pub struct AuthUser {
    pub user: User,
}

impl<S> FromRequestParts<S> for AuthUser
where
    S: Send + Sync,
    MeghAuthState: FromRef<S>,
{
    type Rejection = AuthError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, AuthError> {
        let session = Session::from_request_parts(parts, state).await.map_err(|(status, message)| AuthError(status, message))?;
        Ok(AuthUser { user: current_user(&session, &MeghAuthState::from_ref(state).pool).await? })
    }
}

/// The user signed in to `session`.
async fn current_user(session: &Session, pool: &sqlx::PgPool) -> Result<User, AuthError> {
    let id: Uuid = session.get(USER_ID).await?.ok_or(AuthError(StatusCode::UNAUTHORIZED, "Not logged in"))?;
    UserRepo::new(pool).get_by_id(id).await?.ok_or(AuthError(StatusCode::UNAUTHORIZED, "User not found"))
}

/// Signs `user_id` in to `session`, under a new session id.
async fn sign_in(session: &Session, user_id: Uuid) -> Result<(), AuthError> {
    session.cycle_id().await?;
    Ok(session.insert(USER_ID, user_id).await?)
}

/// A failure the client sees only as a status and a fixed message. Any other error is logged and answers 500.
pub struct AuthError(StatusCode, &'static str);

impl AuthError {
    const INVALID_CREDENTIALS: Self = Self(StatusCode::UNAUTHORIZED, "invalid credentials");
}

impl<E: std::error::Error> From<E> for AuthError {
    fn from(cause: E) -> Self {
        internal(cause)
    }
}

/// `upsert` returns no row for a deleted user.
fn disabled(error: sqlx::Error) -> AuthError {
    match error {
        sqlx::Error::RowNotFound => AuthError(StatusCode::FORBIDDEN, "this account is disabled"),
        other => other.into(),
    }
}

fn internal(cause: impl std::fmt::Display) -> AuthError {
    tracing::error!("{cause}");
    AuthError(StatusCode::INTERNAL_SERVER_ERROR, "internal error")
}

impl IntoResponse for AuthError {
    fn into_response(self) -> Response {
        (self.0, Json(serde_json::json!({ "error": self.1 }))).into_response()
    }
}

/// A provider that failed or answered badly, which the client is only told as "sign-in failed".
fn bad_gateway(cause: impl std::fmt::Display) -> AuthError {
    tracing::error!("{cause}");
    AuthError(StatusCode::BAD_GATEWAY, "sign-in failed")
}

/// Builds an Axum router with all authentication and session endpoints. Writes from another site are refused
/// ([`CsrfLayer`]: `Sec-Fetch-Site` and `Origin`, no state); use [`auth_router_with_csrf`] to trust an origin.
pub fn auth_router(state: MeghAuthState) -> Router {
    auth_router_with_csrf(state, CsrfLayer::new())
}

/// [`auth_router`] with a caller-configured [`CsrfLayer`], for a web app served from another origin than this router
/// (`csrf.add_trusted_origin("https://xyz.com")`).
pub fn auth_router_with_csrf(state: MeghAuthState, csrf: CsrfLayer) -> Router {
    Router::new()
        .route("/auth/{provider}", get(oauth_login))
        .route("/auth/{provider}/login", get(oauth_login))
        .route("/auth/{provider}/callback", get(oauth_callback))
        .route("/auth/{provider}/connect", get(oauth_connect))
        .route("/auth/{provider}/disconnect", post(oauth_disconnect))
        .route("/auth/{provider}/revoke", post(oauth_revoke))
        .route("/auth/{provider}/token", get(oauth_callback))
        .route("/auth/me", get(auth_me))
        .route("/auth/logout", post(auth_logout))
        .route_layer(csrf)
        .with_state(state)
}

/// A `POST <path>` route for email/password sign-in, CSRF-protected like [`auth_router`]. Merge it with
/// [`auth_router`], which provides `/auth/me`, `/auth/logout` and the session this route needs. Never creates an account.
pub fn basic_login_router(state: MeghAuthState, path: &str) -> Router {
    basic_login_router_with_csrf(state, path, CsrfLayer::new())
}

/// [`basic_login_router`] with a caller-configured [`CsrfLayer`].
pub fn basic_login_router_with_csrf(state: MeghAuthState, path: &str, csrf: CsrfLayer) -> Router {
    Router::new().route(path, post(basic_login)).route_layer(csrf).with_state(state)
}

/// Response returned by a successful [`basic_login_router`] sign-in.
#[derive(Debug, Serialize)]
pub struct BasicLoginResponse {
    pub user: User,
    pub memberships: Vec<crate::tenant::Member>,
}

async fn basic_login(
    State(state): State<MeghAuthState>,
    session: Session,
    credentials: Option<TypedHeader<Authorization<Basic>>>,
) -> Result<Json<BasicLoginResponse>, AuthError> {
    let TypedHeader(Authorization(basic)) = credentials.ok_or(AuthError::INVALID_CREDENTIALS)?;
    let user = match UserRepo::new(&state.pool).verify_password(basic.username(), basic.password()).await {
        Err(PasswordError::UserNotFound | PasswordError::NoPassword | PasswordError::WrongPassword) => return Err(AuthError::INVALID_CREDENTIALS),
        other => other?,
    };
    let memberships = Tenants::new(state.pool.clone()).memberships(user.id).await?;
    sign_in(&session, user.id).await?;
    Ok(Json(BasicLoginResponse { user, memberships }))
}

/// How an OAuth flow starts: where one that does not end in a popup returns to, and a connect's extra scopes.
#[derive(Debug, Deserialize)]
pub struct FlowQuery {
    return_to: Option<String>,
    #[serde(default)]
    scope: Vec<String>,
    /// The tenant a connect is for.
    tenant_id: Option<Uuid>,
}

/// The `state` of a connect starts with this, which is how its callback tells it from a login.
const CONNECT_STATE: &str = "connect.";

/// Initiates OAuth login redirection for a given provider (e.g. `/auth/google`).
pub async fn oauth_login(
    State(state): State<MeghAuthState>,
    Path(provider_id): Path<String>,
    ListQuery(query): ListQuery<FlowQuery>,
    session: Session,
) -> Result<Redirect, AuthError> {
    authorize(&state, &provider_id, &session, query.return_to, ("", "select_account"), Vec::new()).await
}

/// Starts the OAuth flow for another account of the signed-in user (`/auth/google/connect?scope=...`, `scope` repeated
/// for each extra scope), or with `tenant_id` for a tenant the user is a member of. The callback stores the account
/// and leaves the session as it is.
pub async fn oauth_connect(
    State(state): State<MeghAuthState>,
    Path(provider_id): Path<String>,
    ListQuery(query): ListQuery<FlowQuery>,
    auth: AuthUser,
    session: Session,
) -> Result<Redirect, AuthError> {
    if let Some(tenant) = query.tenant_id {
        member_of(&state, &auth.user, tenant).await?;
    }
    session.insert(&tenant_key(&provider_id), query.tenant_id).await?;
    authorize(&state, &provider_id, &session, query.return_to, (CONNECT_STATE, "consent"), query.scope).await
}

/// Sends the browser to the provider with PKCE: its default scopes and `scopes`. The `(state prefix, prompt)` pair
/// sets how the flow differs. The attempt is kept in the session for the callback.
async fn authorize(
    state: &MeghAuthState,
    provider_id: &str,
    session: &Session,
    return_to: Option<String>,
    (prefix, prompt): (&str, &str),
    scopes: Vec<String>,
) -> Result<Redirect, AuthError> {
    let provider = state.provider(provider_id)?;
    let csrf = CsrfToken::new(format!("{prefix}{}", CsrfToken::new_random().secret()));
    let (challenge, verifier) = PkceCodeChallenge::new_random_sha256();
    let return_to = return_to.filter(|page| state.is_own_page(page));
    session.insert(&attempt_key(provider_id), (csrf.secret(), verifier.secret(), return_to)).await?;
    let (url, _) = state
        .client(provider)?
        .authorize_url(|| csrf)
        .add_scopes(provider.default_scopes.iter().cloned().chain(scopes).map(Scope::new))
        .set_pkce_challenge(challenge)
        .add_extra_param("access_type", "offline")
        .add_extra_param("include_granted_scopes", "true")
        .add_extra_param("prompt", prompt)
        .url();
    Ok(Redirect::to(url.as_str()))
}

/// Handles the OAuth redirect callback from providers: a login signs the user in, a connect adds the account.
pub async fn oauth_callback(
    State(state): State<MeghAuthState>,
    Path(provider_id): Path<String>,
    Query(query): Query<OAuthCallbackQuery>,
    session: Session,
) -> Result<Response, AuthError> {
    let connecting = query.state.as_deref().is_some_and(|state| state.starts_with(CONNECT_STATE));
    let (verifier, return_to) = take_attempt(&session, &provider_id, query.state).await?;
    if let Some(error) = query.error {
        tracing::error!("sign-in was not completed: {error} ({})", query.error_description.unwrap_or_default());
        return Err(AuthError(StatusCode::BAD_REQUEST, "sign-in was not completed"));
    }
    let code = query.code.ok_or(AuthError(StatusCode::BAD_REQUEST, "Missing code in callback"))?;

    let provider = state.provider(&provider_id)?;
    let tokens = state
        .client(provider)?
        .exchange_code(AuthorizationCode::new(code))
        .set_pkce_verifier(verifier)
        .request_async(&oauth_http_client(state.http_client.clone()))
        .await
        .map_err(bad_gateway)?;
    let userinfo_url = provider.userinfo_url.as_deref().ok_or(AuthError(StatusCode::INTERNAL_SERVER_ERROR, "invalid provider configuration"))?;
    let info = fetch_user_info(&state.http_client, userinfo_url, tokens.access_token().secret()).await.map_err(bad_gateway)?;
    if info.email_verified != Some(true) {
        return Err(AuthError(StatusCode::FORBIDDEN, "the provider has not verified this email"));
    }

    let payload = if connecting {
        let tenant: Option<Uuid> = session.remove(&tenant_key(&provider_id)).await?.flatten();
        let account = match tenant {
            Some(tenant) => save_tenant_account(&state, &provider_id, tenant, info, &tokens).await?,
            None => save_own_account(&state, &session, &provider_id, info, &tokens).await?,
        };
        let account = AccountView { account_id: account.account_id, provider: account.provider, email: account.email };
        serde_json::json!({ "type": "oauth_connect_success", "account": account })
    } else {
        let login = UpsertUserInput {
            provider: provider_id.clone(),
            account_id: info.subject.clone(),
            email: info.email.clone(),
            display_name: info.name.clone(),
            photo_url: info.picture.clone(),
        };
        let user = UserRepo::new(&state.pool).upsert(&login).await.map_err(disabled)?;
        save_account(&state, &provider_id, info, &tokens).await?;
        give_a_tenant(&state, &user).await?;
        sign_in(&session, user.id).await?;
        serde_json::json!({ "type": "oauth_success", "user": user })
    };
    Ok(popup(&state, payload, return_to))
}

/// A user with no membership gets a tenant of their own, named after their email.
async fn give_a_tenant(state: &MeghAuthState, user: &User) -> Result<(), AuthError> {
    let tenants = Tenants::new(state.pool.clone());
    if tenants.memberships(user.id).await?.is_empty() {
        tenants.create(&user.email, user.id).await?;
    }
    Ok(())
}

/// Errs with `403` unless `user` is a member of `tenant`.
async fn member_of(state: &MeghAuthState, user: &User, tenant: Uuid) -> Result<(), AuthError> {
    let memberships = Tenants::new(state.pool.clone()).memberships(user.id).await?;
    let member = memberships.iter().any(|membership| membership.tenant_id == tenant);
    member.then_some(()).ok_or(AuthError(StatusCode::FORBIDDEN, "you are not a member of that tenant"))
}

/// Saves an account of the signed-in user's own: its email must be theirs.
async fn save_own_account(state: &MeghAuthState, session: &Session, provider: &str, info: OAuthUserInfo, tokens: &BasicTokenResponse) -> Result<ConnectedAccount, AuthError> {
    let user = current_user(session, &state.pool).await?;
    if !user.email.eq_ignore_ascii_case(&info.email) {
        return Err(AuthError(StatusCode::FORBIDDEN, "that account's email is not yours"));
    }
    save_account(state, provider, info, tokens).await
}

/// Saves any account as `tenant`'s, unless another tenant has it (`409`).
async fn save_tenant_account(state: &MeghAuthState, provider: &str, tenant: Uuid, info: OAuthUserInfo, tokens: &BasicTokenResponse) -> Result<ConnectedAccount, AuthError> {
    let owner: Option<Option<Uuid>> = sqlx::query_scalar("SELECT tenant_id FROM connected_accounts WHERE account_id = $1 AND provider = $2 AND deleted_at IS NULL")
        .bind(&info.subject)
        .bind(provider)
        .fetch_optional(&state.pool)
        .await?;
    if owner.flatten().is_some_and(|owner| owner != tenant) {
        return Err(AuthError(StatusCode::CONFLICT, "another tenant has connected that account"));
    }
    let account = save_account(state, provider, info, tokens).await?;
    sqlx::query("UPDATE connected_accounts SET tenant_id = $3, auth = 'oauth', deleted_at = NULL WHERE account_id = $1 AND provider = $2")
        .bind(&account.account_id)
        .bind(provider)
        .bind(tenant)
        .execute(&state.pool)
        .await?;
    Ok(account)
}

async fn save_account(state: &MeghAuthState, provider: &str, info: OAuthUserInfo, tokens: &BasicTokenResponse) -> Result<ConnectedAccount, AuthError> {
    let tokens = OAuth2Tokens::from(tokens);
    let account = ConnectedAccount {
        account_id: info.subject,
        provider: provider.to_string(),
        email: Some(info.email),
        access_token: tokens.access_token,
        refresh_token: tokens.refresh_token,
        token_type: Some("Bearer".to_string()),
        expiry: tokens.token_expires_at,
        created_at: None,
        updated_at: None,
        disconnected_at: None,
    };
    Ok(state.accounts().save(&account).await?)
}

/// The page the flow ends on: in a popup it posts `payload` to the opening page and closes itself, otherwise it goes
/// on to the page the flow was started from, or the page after login.
pub(crate) fn popup(state: &MeghAuthState, payload: serde_json::Value, return_to: Option<String>) -> Response {
    let target = state.web_origin.as_deref().unwrap_or(&state.app_origin);
    let html = format!(
        r#"<!doctype html><meta charset="utf-8"><body data-payload="{}" data-target="{}" data-fallback="{}"><script>
var d=document.body.dataset;
if(window.opener){{window.opener.postMessage(JSON.parse(d.payload),d.target);window.close();}}
else if(d.fallback){{window.location.href=d.fallback;}}
</script>"#,
        html_escape::encode_double_quoted_attribute(&payload.to_string()),
        html_escape::encode_double_quoted_attribute(target),
        html_escape::encode_double_quoted_attribute(return_to.as_deref().unwrap_or(&state.redirect_after_login)),
    );
    Html(html).into_response()
}

/// What a connect tells the page that started it: which account, never its tokens.
#[derive(Serialize)]
struct AccountView {
    account_id: String,
    provider: String,
    email: Option<String>,
}

/// The signed-in user's account `account_id`, as the request names it.
#[derive(Debug, Deserialize)]
pub struct AccountRequest {
    account_id: String,
}

/// The user's account at the provider: one whose email is the user's. Anyone else's is not found.
async fn own_account(state: &MeghAuthState, provider: &str, account_id: &str, user: &User) -> Result<ConnectedAccount, AuthError> {
    state
        .accounts()
        .get(account_id, provider)
        .await?
        .filter(|account| account.email.as_deref().is_some_and(|email| email.eq_ignore_ascii_case(&user.email)))
        .ok_or(AuthError(StatusCode::NOT_FOUND, "no such account"))
}

/// Disconnects the signed-in user's account `account_id`: its token is revoked at the provider (best effort) and the
/// account is marked disconnected.
pub async fn oauth_disconnect(
    State(state): State<MeghAuthState>,
    Path(provider_id): Path<String>,
    auth: AuthUser,
    Json(request): Json<AccountRequest>,
) -> Result<StatusCode, AuthError> {
    let account = own_account(&state, &provider_id, &request.account_id, &auth.user).await?;
    let _ = state.revoke(&provider_id, &account.access_token).await;
    state.accounts().disconnect(&account.account_id, &provider_id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Revokes the stored token of the signed-in user's account `account_id` at the provider (RFC 7009); the account
/// stays connected.
pub async fn oauth_revoke(
    State(state): State<MeghAuthState>,
    Path(provider_id): Path<String>,
    auth: AuthUser,
    Json(request): Json<AccountRequest>,
) -> Result<StatusCode, AuthError> {
    let account = own_account(&state, &provider_id, &request.account_id, &auth.user).await?;
    state.revoke(&provider_id, &account.access_token).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Returns the current authenticated user and their memberships.
pub async fn auth_me(State(state): State<MeghAuthState>, auth: AuthUser) -> Result<Json<AuthMeResponse>, AuthError> {
    let memberships = Tenants::new(state.pool.clone()).memberships(auth.user.id).await?;
    Ok(Json(AuthMeResponse { user: auth.user, memberships }))
}

/// Ends the current session and clears the JWT cookie (a token already issued stays valid until it expires).
pub async fn auth_logout(session: Session, jar: CookieJar) -> Result<(CookieJar, Json<serde_json::Value>), AuthError> {
    session.flush().await?;
    Ok((jar.remove(Cookie::build(JWT_COOKIE).path("/")), Json(serde_json::json!({ "status": "ok" }))))
}


fn attempt_key(provider: &str) -> String {
    format!("oauth:{provider}")
}

/// Where the session keeps the tenant a connect is for.
fn tenant_key(provider: &str) -> String {
    format!("oauth:{provider}:tenant")
}

/// The PKCE verifier and the page to return to of the login this callback answers. The stored attempt is consumed, so
/// a callback works once, and the returned `state` must equal the one that was sent.
async fn take_attempt(session: &Session, provider: &str, returned: Option<String>) -> Result<(PkceCodeVerifier, Option<String>), AuthError> {
    let stored: Option<(String, String, Option<String>)> = session.remove(&attempt_key(provider)).await?;
    let (state, verifier, return_to) = stored.ok_or(AuthError(StatusCode::BAD_REQUEST, "no login in progress"))?;
    let matches = returned.is_some_and(|returned| CsrfToken::new(state) == CsrfToken::new(returned));
    matches.then(|| (PkceCodeVerifier::new(verifier), return_to)).ok_or(AuthError(StatusCode::FORBIDDEN, "state mismatch"))
}
