//! Ready-to-mount Axum router, session cookie handlers, and AuthUser extractor for Megh Gateway.

use std::collections::HashMap;
use std::sync::Arc;

use axum::{
    async_trait,
    extract::{FromRequestParts, Path, Query, State},
    http::{header, request::Parts, HeaderMap, StatusCode},
    response::{Html, IntoResponse, Redirect, Response},
    routing::{get, post},
    Json, Router,
};
use chrono::Duration;
use oauth2::TokenResponse;
use serde::{Deserialize, Serialize};

use crate::auth::oauth::{
    build_authorization_url, fetch_user_info, AuthUrlOptions, CsrfToken, OAuthProviderConfig,
    RedirectUrl,
};
use crate::auth::user::{UpsertUserInput, User, UserRepo};
use crate::connection::{ConnectionData, ConnectionRepo, FullConnection, OAuth2Tokens};
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
            http_client: reqwest::Client::new(),
        }
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

#[async_trait]
impl<S> FromRequestParts<S> for AuthUser
where
    S: Send + Sync,
    MeghAuthState: FromRef<S>,
{
    type Rejection = (StatusCode, &'static str);

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let auth_state = MeghAuthState::from_ref(state);
        let token = extract_cookie(&parts.headers, &auth_state.cookie_name)
            .ok_or((StatusCode::UNAUTHORIZED, "Missing session cookie"))?;

        let session_repo = SessionRepo::new(&auth_state.pool);
        let session = session_repo
            .find_valid_by_token(&token)
            .await
            .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Database query failed"))?
            .ok_or((StatusCode::UNAUTHORIZED, "Invalid or expired session"))?;

        let user_repo = UserRepo::new(&auth_state.pool);
        let user = user_repo
            .get_by_id(session.user_id)
            .await
            .map_err(|_| (StatusCode::INTERNAL_SERVER_ERROR, "Database query failed"))?
            .ok_or((StatusCode::UNAUTHORIZED, "User not found"))?;

        Ok(AuthUser { user, session })
    }
}

/// Trait to extract MeghAuthState from application state.
pub trait FromRef<T> {
    fn from_ref(input: &T) -> Self;
}

impl FromRef<MeghAuthState> for MeghAuthState {
    fn from_ref(input: &MeghAuthState) -> Self {
        input.clone()
    }
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
    Router::new()
        .route("/auth/:provider", get(oauth_login))
        .route("/auth/:provider/login", get(oauth_login))
        .route("/auth/:provider/callback", get(oauth_callback))
        .route("/auth/:provider/token", get(oauth_callback))
        .route("/auth/me", get(auth_me))
        .route("/auth/logout", post(auth_logout))
        .with_state(state)
}

/// Initiates OAuth login redirection for a given provider (e.g. `/auth/google`).
pub async fn oauth_login(
    State(state): State<MeghAuthState>,
    Path(provider_id): Path<String>,
) -> Result<Redirect, (StatusCode, String)> {
    let provider = state
        .providers
        .get(&provider_id)
        .ok_or_else(|| (StatusCode::NOT_FOUND, format!("Provider '{provider_id}' not configured")))?;

    let callback_url = provider.redirect_url.clone().unwrap_or_else(|| {
        format!("{}/auth/{provider_id}/token", state.app_origin.trim_end_matches('/'))
    });
    let redirect_url = RedirectUrl::new(callback_url)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("Invalid callback URL: {e}")))?;

    let client = provider
        .build_client(Some(redirect_url))
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let csrf = CsrfToken::new_random();
    let auth_url = build_authorization_url(
        &client,
        csrf,
        AuthUrlOptions {
            scopes: &provider.default_scopes.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
            pkce: None,
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
    headers: HeaderMap,
) -> Result<Response, (StatusCode, String)> {
    if let Some(err) = query.error {
        let desc = query.error_description.unwrap_or_default();
        return Err((StatusCode::BAD_REQUEST, format!("OAuth error: {err} ({desc})")));
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
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("Invalid callback URL: {e}")))?;

    let client = provider
        .build_client(Some(redirect_url))
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Exchange authorization code for tokens
    let token_response = client
        .exchange_code(oauth2::AuthorizationCode::new(code))
        .request_async(oauth2::reqwest::async_http_client)
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, format!("Token exchange failed: {e}")))?;

    // Fetch user profile
    let userinfo_url = provider
        .userinfo_url
        .as_deref()
        .ok_or_else(|| (StatusCode::INTERNAL_SERVER_ERROR, "userinfo_url missing".to_string()))?;

    let user_info = fetch_user_info(
        &state.http_client,
        userinfo_url,
        token_response.access_token().secret(),
    )
    .await
    .map_err(|e| (StatusCode::BAD_GATEWAY, format!("Fetch userinfo failed: {e}")))?;

    // Upsert User identity
    let user_repo = UserRepo::new(&state.pool);
    let user = user_repo
        .upsert(&UpsertUserInput {
            subject: format!("{provider_id}:{}", user_info.subject),
            email: user_info.email.clone(),
            display_name: user_info.name,
            photo_url: user_info.picture,
        })
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("User upsert failed: {e}")))?;

    // Save Connection credentials
    let oauth_tokens = OAuth2Tokens::from(&token_response);
    let connection_repo = ConnectionRepo::new(&state.pool);
    let _ = connection_repo
        .upsert(&FullConnection {
            data: ConnectionData {
                user_id: user.id,
                org_id: None,
                provider: provider_id,
                provider_account_id: user_info.subject,
                scopes: provider.default_scopes.clone(),
                metadata: serde_json::json!({ "email": user_info.email }),
            },
            tokens: oauth_tokens,
        })
        .await;

    // Create active Session
    let user_agent = headers
        .get(header::USER_AGENT)
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");
    let session_repo = SessionRepo::new(&state.pool);
    let created_session = session_repo
        .create(
            user.id,
            state.session_duration,
            user_agent,
            "",
            serde_json::json!({}),
        )
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("Session creation failed: {e}")))?;

    // Build Cookie header and Popup / Redirect response
    let cookie_val = format!(
        "{}={}; Path=/; HttpOnly; SameSite=Lax; Max-Age={}",
        state.cookie_name,
        created_session.plaintext_token,
        state.session_duration.num_seconds()
    );

    let payload = serde_json::json!({
        "type": "oauth_success",
        "user": user,
    });
    let payload_str = serde_json::to_string(&payload).unwrap_or_else(|_| "{}".to_string());
    let fallback_str = serde_json::to_string(&state.redirect_after_login).unwrap_or_else(|_| "\"/\"".to_string());

    let html = format!(
        r#"<!doctype html><meta charset="utf-8"><script>
(function(){{
var d={payload_str},f={fallback_str};
if(window.opener){{window.opener.postMessage(d,"*");window.close();}}
else if(f){{window.location.href=f;}}
}})();
</script>"#
    );

    let response = (
        StatusCode::OK,
        [
            (header::SET_COOKIE, cookie_val),
            (header::CONTENT_TYPE, "text/html; charset=utf-8".to_string()),
        ],
        Html(html),
    )
        .into_response();

    Ok(response)
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

    let clear_cookie = format!("{}=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0", state.cookie_name);
    (
        StatusCode::OK,
        [(header::SET_COOKIE, clear_cookie)],
        Json(serde_json::json!({ "status": "ok" })),
    )
}
