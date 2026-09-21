//! Ready-to-mount Axum router, session cookie handlers, and AuthUser extractor for Megh Gateway.

use std::collections::HashMap;
use std::sync::Arc;

pub use axum::extract::FromRef;
use axum::{
    extract::{FromRequestParts, Path, Query, State},
    http::{header, request::Parts, HeaderMap, StatusCode},
    response::{AppendHeaders, Html, IntoResponse, Redirect, Response},
    routing::{get, post},
    Json, Router,
};
use chrono::Duration;
use oauth2::basic::BasicTokenResponse;
use oauth2::{AuthorizationCode, TokenResponse};
use serde::{Deserialize, Serialize};

use crate::auth::flow::{self, Flow};
use crate::auth::oauth::{
    build_authorization_url, fetch_user_info, oauth_http_client, AuthUrlOptions, OAuthProviderConfig, OAuthUserInfo,
    ProviderClient, RedirectUrl,
};
use crate::account::{ConnectedAccount, ConnectedAccountRepo, OAuth2Tokens};
use crate::auth::user::{UpsertUserInput, User, UserRepo};
use crate::auth::{connect, CsrfLayer};
use crate::session::{Session, SessionExt, SessionRepo, SessionView};

/// Shared state required by the Megh authentication HTTP router.
#[derive(Clone)]
pub struct MeghAuthState {
    pub pool: sqlx::PgPool,
    pub cookie_name: String,
    pub session_duration: Duration,
    pub redirect_after_login: String,
    pub app_origin: String,
    pub providers: Arc<HashMap<String, OAuthProviderConfig>>,
    pub http_client: reqwest::Client,
    pub csrf: CsrfLayer,
}

impl MeghAuthState {
    pub fn new(pool: sqlx::PgPool) -> Self {
        Self {
            pool,
            cookie_name: "kyrios_session".to_string(),
            session_duration: Duration::days(30),
            redirect_after_login: "/".to_string(),
            app_origin: "http://localhost:8080".to_string(),
            providers: Arc::new(HashMap::new()),
            http_client: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(std::time::Duration::from_secs(10))
                .build()
                .expect("static HTTP client configuration"),
            csrf: CsrfLayer::new(),
        }
    }

    pub fn with_csrf(mut self, csrf: CsrfLayer) -> Self {
        self.csrf = csrf;
        self
    }

    pub fn with_cookie_name(mut self, cookie_name: impl Into<String>) -> Self {
        self.cookie_name = cookie_name.into();
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

    pub fn add_provider(mut self, provider: OAuthProviderConfig) -> Self {
        Arc::make_mut(&mut self.providers).insert(provider.provider_id.clone(), provider);
        self
    }
}

/// Response returned by `/auth/me`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthMeResponse {
    pub user: User,
    pub session: SessionView,
}

/// Query parameters passed in OAuth callback redirects.
#[derive(Debug, Deserialize)]
pub struct OAuthCallbackQuery {
    pub code: Option<String>,
    pub state: Option<String>,
    pub error: Option<String>,
    pub error_description: Option<String>,
}

/// Axum extractor that injects the authenticated User and Session into route handlers.
#[derive(Debug, Clone)]
pub struct AuthUser {
    pub user: User,
    pub session: Session,
}

impl<S> FromRequestParts<S> for AuthUser
where
    S: Send + Sync,
    MeghAuthState: FromRef<S>,
{
    type Rejection = (StatusCode, &'static str);

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        authenticate(&MeghAuthState::from_ref(state), &parts.headers).await
    }
}

/// Resolves the session cookie in `headers` to the signed-in user.
pub async fn authenticate(state: &MeghAuthState, headers: &HeaderMap) -> Result<AuthUser, (StatusCode, &'static str)> {
    let token = extract_cookie(headers, &state.cookie_name).ok_or((StatusCode::UNAUTHORIZED, "Missing session cookie"))?;
    let session = SessionRepo::new(&state.pool)
        .find_valid_by_token(&token)
        .await
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Database query failed"))?
        .ok_or((StatusCode::UNAUTHORIZED, "Invalid or expired session"))?;
    let user = UserRepo::new(&state.pool)
        .get_by_id(session.user_id)
        .await
        .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Database query failed"))?
        .ok_or((StatusCode::UNAUTHORIZED, "User not found"))?;
    Ok(AuthUser { user, session })
}

/// Extracts a named cookie value from HTTP request headers.
pub fn extract_cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    let prefix = format!("{name}=");
    headers
        .get(header::COOKIE)?
        .to_str()
        .ok()?
        .split(';')
        .find_map(|cookie| {
            let cookie = cookie.trim();
            if cookie.starts_with(&prefix) {
                Some(cookie[prefix.len()..].to_string())
            } else {
                None
            }
        })
}

/// Builds an Axum router with all authentication and session endpoints.
pub fn auth_router(state: MeghAuthState) -> Router {
    let csrf = state.csrf.clone();
    Router::new()
        .route("/auth/{provider}", get(oauth_login))
        .route("/auth/{provider}/login", get(oauth_login))
        .route("/auth/{provider}/callback", get(oauth_callback))
        .route("/auth/{provider}/token", get(oauth_callback))
        .route("/auth/{provider}/connect", get(connect::oauth_connect))
        .route("/auth/{provider}/disconnect", post(connect::oauth_disconnect))
        .route("/auth/{provider}/revoke", post(connect::oauth_revoke))
        .route("/auth/me", get(auth_me))
        .route("/auth/logout", post(auth_logout))
        .with_state(state)
        .layer(csrf)
}

/// Whether cookies set for this deployment carry `Secure` (the app is served over https).
pub(super) fn secure(state: &MeghAuthState) -> bool {
    state.app_origin.starts_with("https://")
}

/// Looks up the provider and builds its OAuth client with the callback URL.
pub(super) fn provider_client<'a>(state: &'a MeghAuthState, provider_id: &str) -> Result<(&'a OAuthProviderConfig, ProviderClient), (StatusCode, String)> {
    let provider = state
        .providers
        .get(provider_id)
        .ok_or_else(|| (StatusCode::NOT_FOUND, format!("Provider '{provider_id}' not configured")))?;
    let callback_url = provider
        .redirect_url
        .clone()
        .unwrap_or_else(|| format!("{}/auth/{provider_id}/token", state.app_origin.trim_end_matches('/')));
    let redirect_url = RedirectUrl::new(callback_url).map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("Invalid callback URL: {e}")))?;
    let client = provider.build_client(Some(redirect_url)).map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok((provider, client))
}

/// Redirects to the provider to start a flow, binding the browser with the `state` and PKCE cookies.
pub(super) fn start_flow(state: &MeghAuthState, provider_id: &str, flow: Flow, extra_scopes: &[String]) -> Result<Response, (StatusCode, String)> {
    let (provider, client) = provider_client(state, provider_id)?;
    let attempt = flow::begin(provider_id, flow, secure(state));
    let scopes: Vec<&str> = provider.default_scopes.iter().chain(extra_scopes).map(String::as_str).collect();
    let auth_url = build_authorization_url(
        &client,
        attempt.csrf,
        AuthUrlOptions {
            scopes: &scopes,
            pkce: Some(&attempt.challenge),
            offline_access: true,
            incremental: true,
            prompt: Some(if flow == Flow::Connect { "consent" } else { "select_account" }),
        },
    );
    let cookies = AppendHeaders(attempt.cookies.map(|c| (header::SET_COOKIE, c)));
    Ok((cookies, Redirect::to(auth_url.as_str())).into_response())
}

/// Initiates OAuth login redirection for a given provider (e.g. `/auth/google`).
pub async fn oauth_login(State(state): State<MeghAuthState>, Path(provider_id): Path<String>) -> Result<Response, (StatusCode, String)> {
    start_flow(&state, &provider_id, Flow::Login, &[])
}

/// Handles the OAuth redirect callback from providers, for both login and connect flows.
pub async fn oauth_callback(
    State(state): State<MeghAuthState>,
    Path(provider_id): Path<String>,
    Query(query): Query<OAuthCallbackQuery>,
    headers: HeaderMap,
) -> Response {
    let cleared = AppendHeaders(flow::clear_cookies(&provider_id).map(|c| (header::SET_COOKIE, c)));
    match callback(&state, &provider_id, query, &headers).await {
        Ok(response) => (cleared, response).into_response(),
        Err((status, message)) => (status, cleared, message).into_response(),
    }
}

async fn callback(state: &MeghAuthState, provider_id: &str, query: OAuthCallbackQuery, headers: &HeaderMap) -> Result<Response, (StatusCode, String)> {
    if let Some(err) = query.error {
        let desc = query.error_description.unwrap_or_default();
        return Err((StatusCode::BAD_REQUEST, format!("OAuth error: {err} ({desc})")));
    }
    let (flow, verifier) = flow::verify(headers, provider_id, query.state.as_deref()).map_err(|(status, message)| (status, message.to_string()))?;
    let code = query.code.ok_or_else(|| (StatusCode::BAD_REQUEST, "Missing code in callback".to_string()))?;
    let (provider, client) = provider_client(state, provider_id)?;

    let tokens = client
        .exchange_code(AuthorizationCode::new(code))
        .set_pkce_verifier(verifier)
        .request_async(&oauth_http_client(state.http_client.clone()))
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, format!("Token exchange failed: {e}")))?;
    let userinfo_url = provider.userinfo_url.as_deref().ok_or_else(|| (StatusCode::INTERNAL_SERVER_ERROR, "userinfo_url missing".to_string()))?;
    let info = fetch_user_info(&state.http_client, userinfo_url, tokens.access_token().secret())
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, format!("Fetch userinfo failed: {e}")))?;

    match flow {
        Flow::Login => login(state, provider_id, headers, &tokens, info).await,
        Flow::Connect => connect::connect_account(state, provider_id, headers, &tokens, &info).await,
    }
}

/// Saves the account's tokens; a token the provider did not resend keeps the stored one.
pub(super) async fn save_account(state: &MeghAuthState, provider_id: &str, info: &OAuthUserInfo, tokens: &BasicTokenResponse) -> Result<ConnectedAccount, sqlx::Error> {
    let tokens = OAuth2Tokens::from(tokens);
    ConnectedAccountRepo::new(&state.pool)
        .save(&ConnectedAccount {
            account_id: info.subject.clone(),
            provider: provider_id.to_string(),
            email: Some(info.email.clone()),
            access_token: tokens.access_token,
            refresh_token: tokens.refresh_token,
            token_type: Some("Bearer".to_string()),
            expiry: tokens.token_expires_at,
            created_at: None,
            updated_at: None,
            disconnected_at: None,
        })
        .await
}

async fn login(state: &MeghAuthState, provider_id: &str, headers: &HeaderMap, tokens: &BasicTokenResponse, info: OAuthUserInfo) -> Result<Response, (StatusCode, String)> {
    let user = UserRepo::new(&state.pool)
        .upsert(&UpsertUserInput {
            provider: provider_id.to_string(),
            account_id: info.subject.clone(),
            email: info.email.clone(),
            display_name: info.name.clone(),
            photo_url: info.picture.clone(),
        })
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("User upsert failed: {e}")))?;
    let _ = save_account(state, provider_id, &info, tokens).await;

    let user_agent = headers.get(header::USER_AGENT).and_then(|h| h.to_str().ok()).unwrap_or("");
    let session = SessionRepo::new(&state.pool)
        .create(user.id, state.session_duration, user_agent, "", serde_json::json!({}))
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("Session creation failed: {e}")))?;
    let cookie = format!(
        "{}={}; Path=/; HttpOnly; SameSite=Lax; Max-Age={}",
        state.cookie_name,
        session.plaintext_token,
        state.session_duration.num_seconds()
    );
    let page = popup_page(&serde_json::json!({ "type": "oauth_success", "user": user }), &state.redirect_after_login);
    Ok((StatusCode::OK, [(header::SET_COOKIE, cookie)], page).into_response())
}

/// The page a popup lands on: posts `payload` to the opener and closes, or redirects to `fallback` without one.
/// The JSON is escaped so provider-supplied text cannot end the script element.
pub(super) fn popup_page(payload: &serde_json::Value, fallback: &str) -> Html<String> {
    let embed = |value: &serde_json::Value| {
        value.to_string().replace('<', "\\u003c").replace('>', "\\u003e").replace('&', "\\u0026").replace('\u{2028}', "\\u2028").replace('\u{2029}', "\\u2029")
    };
    Html(format!(
        r#"<!doctype html><meta charset="utf-8"><script>
(function(){{
var d={},f={};
if(window.opener){{window.opener.postMessage(d,"*");window.close();}}
else if(f){{window.location.href=f;}}
}})();
</script>"#,
        embed(payload),
        embed(&serde_json::json!(fallback))
    ))
}

/// Returns the current authenticated user and session.
pub async fn auth_me(auth: AuthUser) -> Json<AuthMeResponse> {
    Json(AuthMeResponse {
        user: auth.user,
        session: auth.session.to_view(),
    })
}

/// Revokes the current session and clears the cookie.
pub async fn auth_logout(
    State(state): State<MeghAuthState>,
    headers: HeaderMap,
) -> impl IntoResponse {
    if let Some(token) = extract_cookie(&headers, &state.cookie_name) {
        let session_repo = SessionRepo::new(&state.pool);
        let _ = session_repo.revoke_by_token(&token).await;
    }

    (
        StatusCode::OK,
        [(header::SET_COOKIE, clear_session_cookie(&state))],
        Json(serde_json::json!({ "status": "ok" })),
    )
}

/// A `Set-Cookie` value that removes the session cookie.
pub(super) fn clear_session_cookie(state: &MeghAuthState) -> String {
    format!("{}=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0", state.cookie_name)
}
