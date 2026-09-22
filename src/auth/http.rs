//! Ready-to-mount Axum router and AuthUser extractor for Megh Gateway. Sessions come from `tower-sessions`:
//! mount the router under a `SessionManagerLayer`.

use std::collections::HashMap;
use std::sync::Arc;

pub use axum::extract::FromRef;
use axum::{
    extract::{FromRequestParts, Path, Query, State},
    http::{request::Parts, StatusCode},
    response::{Html, IntoResponse, Redirect, Response},
    middleware,
    routing::{get, post},
    Json, Router,
};
use axum_extra::extract::cookie::{Cookie, CookieJar};
use axum_extra::headers::{authorization::Basic, Authorization};
use axum_extra::TypedHeader;
use axum_tower_sessions_csrf::{get_or_create_token, CsrfMiddleware};
use oauth2::TokenResponse;
use serde::{Deserialize, Serialize};
use tower_sessions::Session;
use uuid::Uuid;

use crate::auth::oauth::{
    build_authorization_url, fetch_user_info, oauth_http_client, AuthUrlOptions, CsrfToken, OAuthProviderConfig,
    PkceCodeChallenge, PkceCodeVerifier,
    RedirectUrl,
};
use crate::account::{ConnectedAccount, ConnectedAccountRepo, Encryptor, OAuth2Tokens};
use crate::auth::token::JWT_COOKIE;
use crate::auth::user::{PasswordError, UpsertUserInput, User, UserRepo};
use crate::org::Orgs;

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
}

/// Response returned by `/auth/me`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthMeResponse {
    pub user: User,
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
    type Rejection = (StatusCode, &'static str);

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let session = Session::from_request_parts(parts, state).await?;
        let user_id: Uuid = session
            .get(USER_ID)
            .await
            .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Session read failed"))?
            .ok_or((StatusCode::UNAUTHORIZED, "Not logged in"))?;

        let user = UserRepo::new(&MeghAuthState::from_ref(state).pool)
            .get_by_id(user_id)
            .await
            .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Database query failed"))?
            .ok_or((StatusCode::UNAUTHORIZED, "User not found"))?;

        Ok(AuthUser { user })
    }
}

/// Builds an Axum router with all authentication and session endpoints.
pub fn auth_router(state: MeghAuthState) -> Router {
    Router::new()
        .route("/auth/{provider}", get(oauth_login))
        .route("/auth/{provider}/login", get(oauth_login))
        .route("/auth/{provider}/callback", get(oauth_callback))
        .route("/auth/{provider}/token", get(oauth_callback))
        .route("/auth/me", get(auth_me))
        .route("/auth/logout", post(auth_logout))
        .route("/auth/csrf-token", get(csrf_token))
        .route_layer(middleware::from_fn(CsrfMiddleware::middleware))
        .with_state(state)
}

/// A `POST <path>` route for email/password sign-in, CSRF-protected. Merge it with [`auth_router`], which
/// provides `/auth/me`, `/auth/logout` and the session/CSRF plumbing this route needs. Never creates an account.
pub fn basic_login_router(state: MeghAuthState, path: &str) -> Router {
    Router::new()
        .route(path, post(basic_login))
        .route_layer(middleware::from_fn(CsrfMiddleware::middleware))
        .with_state(state)
}

/// Response returned by a successful [`basic_login_router`] sign-in.
#[derive(Debug, Serialize)]
pub struct BasicLoginResponse {
    pub user: User,
    pub memberships: Vec<crate::org::Member>,
}

async fn basic_login(
    State(state): State<MeghAuthState>,
    session: Session,
    credentials: Option<TypedHeader<Authorization<Basic>>>,
) -> Result<Json<BasicLoginResponse>, LoginError> {
    let TypedHeader(Authorization(basic)) = credentials.ok_or(LoginError::InvalidCredentials)?;

    let user = UserRepo::new(&state.pool).verify_password(basic.username(), basic.password()).await?;
    let memberships = Orgs::new(state.pool.clone()).memberships(user.id).await.map_err(|e| login_failed("login failed", e))?;
    session.cycle_id().await.map_err(|e| login_failed("could not start the session", e))?;
    session.insert(USER_ID, user.id).await.map_err(|e| login_failed("could not start the session", e))?;

    Ok(Json(BasicLoginResponse { user, memberships }))
}

/// A login failure as the client sees it: the reason is either "invalid credentials" or a fixed 500 message,
/// never the underlying database or hashing error.
enum LoginError {
    InvalidCredentials,
    Failed(&'static str),
}

fn login_failed(message: &'static str, cause: impl std::fmt::Display) -> LoginError {
    tracing::error!("{message}: {cause}");
    LoginError::Failed(message)
}

impl From<PasswordError> for LoginError {
    fn from(error: PasswordError) -> Self {
        match error {
            PasswordError::UserNotFound | PasswordError::NoPassword | PasswordError::WrongPassword => Self::InvalidCredentials,
            e => login_failed("login failed", e),
        }
    }
}

impl IntoResponse for LoginError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            Self::InvalidCredentials => (StatusCode::UNAUTHORIZED, "invalid credentials"),
            Self::Failed(message) => (StatusCode::INTERNAL_SERVER_ERROR, message),
        };
        (status, Json(serde_json::json!({ "error": message }))).into_response()
    }
}

/// Initiates OAuth login redirection for a given provider (e.g. `/auth/google`).
pub async fn oauth_login(
    State(state): State<MeghAuthState>,
    Path(provider_id): Path<String>,
    session: Session,
) -> Result<Redirect, (StatusCode, String)> {
    let provider = state
        .providers
        .get(&provider_id)
        .ok_or_else(|| (StatusCode::NOT_FOUND, format!("Provider '{provider_id}' not configured")))?;

    let callback_url = provider.redirect_url.clone().unwrap_or_else(|| {
        format!("{}/auth/{provider_id}/token", state.app_origin.trim_end_matches('/'))
    });
    let redirect_url = RedirectUrl::new(callback_url)
        .map_err(|e| failed(StatusCode::INTERNAL_SERVER_ERROR, "invalid callback URL", e))?;

    let client = provider
        .build_client(Some(redirect_url))
        .map_err(|e| failed(StatusCode::INTERNAL_SERVER_ERROR, "invalid provider configuration", e))?;

    let csrf = CsrfToken::new_random();
    let (challenge, verifier) = PkceCodeChallenge::new_random_sha256();
    session
        .insert(&attempt_key(&provider_id), (csrf.secret(), verifier.secret()))
        .await
        .map_err(|e| failed(StatusCode::INTERNAL_SERVER_ERROR, "could not start the login", e))?;
    let auth_url = build_authorization_url(
        &client,
        csrf,
        AuthUrlOptions {
            scopes: &provider.default_scopes.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
            pkce: Some(&challenge),
            offline_access: true,
            incremental: true,
            prompt: Some("select_account"),
        },
    );

    Ok(Redirect::to(auth_url.as_str()))
}

/// Handles the OAuth redirect callback from providers.
pub async fn oauth_callback(
    State(state): State<MeghAuthState>,
    Path(provider_id): Path<String>,
    Query(query): Query<OAuthCallbackQuery>,
    session: Session,
) -> Result<Response, (StatusCode, String)> {
    let verifier = take_attempt(&session, &provider_id, query.state).await?;
    if let Some(err) = query.error {
        let desc = query.error_description.unwrap_or_default();
        return Err(failed(StatusCode::BAD_REQUEST, "sign-in was not completed", format!("{err} ({desc})")));
    }

    let code = query
        .code
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "Missing code in callback".to_string()))?;

    let provider = state
        .providers
        .get(&provider_id)
        .ok_or_else(|| (StatusCode::NOT_FOUND, format!("Provider '{provider_id}' not configured")))?;

    let callback_url = provider.redirect_url.clone().unwrap_or_else(|| {
        format!("{}/auth/{provider_id}/token", state.app_origin.trim_end_matches('/'))
    });
    let redirect_url = RedirectUrl::new(callback_url)
        .map_err(|e| failed(StatusCode::INTERNAL_SERVER_ERROR, "invalid callback URL", e))?;

    let client = provider
        .build_client(Some(redirect_url))
        .map_err(|e| failed(StatusCode::INTERNAL_SERVER_ERROR, "invalid provider configuration", e))?;

    // Exchange authorization code for tokens
    let token_response = client
        .exchange_code(oauth2::AuthorizationCode::new(code))
        .set_pkce_verifier(verifier)
        .request_async(&oauth_http_client(state.http_client.clone()))
        .await
        .map_err(|e| failed(StatusCode::BAD_GATEWAY, "sign-in failed", e))?;

    // Fetch user profile
    let userinfo_url = provider
        .userinfo_url
        .as_deref()
        .ok_or_else(|| failed(StatusCode::INTERNAL_SERVER_ERROR, "invalid provider configuration", "userinfo_url missing"))?;

    let user_info = fetch_user_info(
        &state.http_client,
        userinfo_url,
        token_response.access_token().secret(),
    )
    .await
    .map_err(|e| failed(StatusCode::BAD_GATEWAY, "sign-in failed", e))?;
    (user_info.email_verified == Some(true))
        .then_some(())
        .ok_or((StatusCode::FORBIDDEN, "the provider has not verified this email".to_string()))?;

    // Upsert User identity
    let user_repo = UserRepo::new(&state.pool);
    let user = user_repo
        .upsert(&UpsertUserInput {
            provider: provider_id.clone(),
            account_id: user_info.subject.clone(),
            email: user_info.email.clone(),
            display_name: user_info.name,
            photo_url: user_info.picture,
        })
        .await
        .map_err(|e| failed(StatusCode::INTERNAL_SERVER_ERROR, "could not save the user", e))?;

    // Save ConnectedAccount credentials
    let oauth_tokens = OAuth2Tokens::from(&token_response);
    let mut account_repo = ConnectedAccountRepo::new(&state.pool);
    if let Some(encryptor) = state.token_encryptor.clone() {
        account_repo = account_repo.with_encryptor(encryptor);
    }
    account_repo
        .save(&ConnectedAccount {
            account_id: user_info.subject,
            provider: provider_id,
            email: Some(user_info.email),
            access_token: oauth_tokens.access_token,
            refresh_token: oauth_tokens.refresh_token,
            token_type: Some("Bearer".to_string()),
            expiry: oauth_tokens.token_expires_at,
            created_at: None,
            updated_at: None,
            disconnected_at: None,
        })
        .await
        .map_err(|e| failed(StatusCode::INTERNAL_SERVER_ERROR, "could not save the account", e))?;

    session
        .cycle_id()
        .await
        .map_err(|e| failed(StatusCode::INTERNAL_SERVER_ERROR, "could not start the session", e))?;
    session
        .insert(USER_ID, user.id)
        .await
        .map_err(|e| failed(StatusCode::INTERNAL_SERVER_ERROR, "could not start the session", e))?;

    let payload = serde_json::json!({ "type": "oauth_success", "user": user }).to_string();
    let target = state.web_origin.as_deref().unwrap_or(&state.app_origin);
    let html = format!(
        r#"<!doctype html><meta charset="utf-8"><body data-payload="{}" data-target="{}" data-fallback="{}"><script>
var d=document.body.dataset;
if(window.opener){{window.opener.postMessage(JSON.parse(d.payload),d.target);window.close();}}
else if(d.fallback){{window.location.href=d.fallback;}}
</script>"#,
        html_escape::encode_double_quoted_attribute(&payload),
        html_escape::encode_double_quoted_attribute(target),
        html_escape::encode_double_quoted_attribute(&state.redirect_after_login),
    );

    Ok(Html(html).into_response())
}

/// Returns the current authenticated user.
pub async fn auth_me(auth: AuthUser) -> Json<AuthMeResponse> {
    Json(AuthMeResponse { user: auth.user })
}

/// Ends the current session and clears the JWT cookie (a token already issued stays valid until it expires).
pub async fn auth_logout(session: Session, jar: CookieJar) -> Result<(CookieJar, Json<serde_json::Value>), (StatusCode, String)> {
    session.flush().await.map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok((jar.remove(Cookie::build(JWT_COOKIE).path("/")), Json(serde_json::json!({ "status": "ok" }))))
}

/// The token a client sends back in the `x-csrf-token` header on every write.
pub async fn csrf_token(session: Session) -> Result<String, (StatusCode, String)> {
    get_or_create_token(&session).await.map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e))
}

fn attempt_key(provider: &str) -> String {
    format!("oauth:{provider}")
}

/// The PKCE verifier of the login this callback answers. The stored attempt is consumed, so a callback works once,
/// and the returned `state` must equal the one that was sent.
async fn take_attempt(session: &Session, provider: &str, returned: Option<String>) -> Result<PkceCodeVerifier, (StatusCode, String)> {
    let stored: Option<(String, String)> = session
        .remove(&attempt_key(provider))
        .await
        .map_err(|e| failed(StatusCode::INTERNAL_SERVER_ERROR, "could not read the session", e))?;
    let (state, verifier) = stored.ok_or((StatusCode::BAD_REQUEST, "no login in progress".to_string()))?;
    let matches = returned.is_some_and(|returned| CsrfToken::new(state) == CsrfToken::new(returned));
    matches.then(|| PkceCodeVerifier::new(verifier)).ok_or((StatusCode::FORBIDDEN, "state mismatch".to_string()))
}

/// A failure the client sees only as a status and a fixed message; the cause is logged.
fn failed(status: StatusCode, message: &'static str, cause: impl std::fmt::Display) -> (StatusCode, String) {
    tracing::error!("{message}: {cause}");
    (status, message.to_string())
}
