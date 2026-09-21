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
use axum_tower_sessions_csrf::{get_or_create_token, CsrfMiddleware};
use oauth2::TokenResponse;
use serde::{Deserialize, Serialize};
use tower_sessions::Session;
use uuid::Uuid;

use crate::auth::oauth::{
    build_authorization_url, fetch_user_info, oauth_http_client, AuthUrlOptions, CsrfToken, OAuthProviderConfig,
    RedirectUrl,
};
use crate::account::{ConnectedAccount, ConnectedAccountRepo, OAuth2Tokens};
use crate::auth::token::JWT_COOKIE;
use crate::auth::user::{UpsertUserInput, User, UserRepo};

/// Shared state required by the Megh authentication HTTP router.
#[derive(Clone)]
pub struct MeghAuthState {
    pub pool: sqlx::PgPool,
    pub redirect_after_login: String,
    pub app_origin: String,
    pub providers: Arc<HashMap<String, OAuthProviderConfig>>,
    pub http_client: reqwest::Client,
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
        }
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
    session: Session,
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
        .request_async(&oauth_http_client(state.http_client.clone()))
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
            provider: provider_id.clone(),
            account_id: user_info.subject.clone(),
            email: user_info.email.clone(),
            display_name: user_info.name,
            photo_url: user_info.picture,
        })
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("User upsert failed: {e}")))?;

    // Save ConnectedAccount credentials
    let oauth_tokens = OAuth2Tokens::from(&token_response);
    let account_repo = ConnectedAccountRepo::new(&state.pool);
    let _ = account_repo
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
        .await;

    session
        .cycle_id()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("Session creation failed: {e}")))?;
    session
        .insert(USER_ID, user.id)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("Session creation failed: {e}")))?;

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
