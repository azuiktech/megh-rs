//! Routes to connect, disconnect and revoke the provider accounts of the signed-in user.

use axum::body::Bytes;
use axum::extract::{Path, RawQuery, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use oauth2::basic::BasicTokenResponse;
use oauth2::url::form_urlencoded;
use serde::Deserialize;

use super::flow::Flow;
use super::http::{authenticate, clear_session_cookie, popup_page, save_account, start_flow, AuthUser, MeghAuthState};
use super::oauth::OAuthUserInfo;
use super::user::User;
use crate::account::{ConnectedAccount, ConnectedAccountRepo};
use crate::session::SessionRepo;

type Failure = (StatusCode, String);

fn internal(error: sqlx::Error) -> Failure {
    (StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
}

/// An account belongs to the user when it is connected and carries the user's email.
fn owns(user: &User, account: &ConnectedAccount) -> bool {
    account.is_connected() && account.email.as_deref().is_some_and(|email| email.eq_ignore_ascii_case(&user.email))
}

/// Starts connecting another provider account, asking for the `scope` values in the query on top of the default scopes.
pub async fn oauth_connect(State(state): State<MeghAuthState>, Path(provider_id): Path<String>, RawQuery(query): RawQuery, _auth: AuthUser) -> Result<Response, Failure> {
    let scopes: Vec<String> = form_urlencoded::parse(query.unwrap_or_default().as_bytes())
        .filter(|(key, _)| key == "scope")
        .map(|(_, value)| value.into_owned())
        .collect();
    start_flow(&state, &provider_id, Flow::Connect, &scopes)
}

/// Finishes a connect flow: the provider account must carry the signed-in user's verified email.
pub(super) async fn connect_account(state: &MeghAuthState, provider_id: &str, headers: &HeaderMap, tokens: &BasicTokenResponse, info: &OAuthUserInfo) -> Result<Response, Failure> {
    let auth = authenticate(state, headers).await.map_err(|(status, message)| (status, message.to_string()))?;
    if info.email_verified != Some(true) || !info.email.eq_ignore_ascii_case(&auth.user.email) {
        return Err((StatusCode::FORBIDDEN, "the account's email does not match the signed-in user".to_string()));
    }
    let account = save_account(state, provider_id, info, tokens).await.map_err(internal)?;
    let payload = serde_json::json!({
        "type": "oauth_connect_success",
        "account": { "account_id": account.account_id, "provider": account.provider, "email": account.email },
    });
    Ok(popup_page(&payload, &state.redirect_after_login).into_response())
}

#[derive(Debug, Deserialize)]
pub struct DisconnectRequest {
    pub account_id: String,
}

/// Disconnects one of the user's accounts, revoking its token at the provider first (best effort).
pub async fn oauth_disconnect(State(state): State<MeghAuthState>, Path(provider_id): Path<String>, auth: AuthUser, Json(request): Json<DisconnectRequest>) -> Result<StatusCode, Failure> {
    let accounts = ConnectedAccountRepo::new(&state.pool);
    let account = accounts
        .get(&request.account_id, &provider_id)
        .await
        .map_err(internal)?
        .filter(|account| owns(&auth.user, account))
        .ok_or((StatusCode::NOT_FOUND, "account not found".to_string()))?;
    let _ = revoke_at_provider(&state, &provider_id, stored_token(&account)).await;
    accounts.disconnect(&account.account_id, &provider_id).await.map_err(internal)?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Default, Deserialize)]
struct RevokeRequest {
    token: Option<String>,
}

/// Revokes the user's tokens at the provider, disconnects the account and signs the user out.
/// A `token` in the body must be one of the account's own.
pub async fn oauth_revoke(State(state): State<MeghAuthState>, Path(provider_id): Path<String>, auth: AuthUser, body: Bytes) -> Result<Response, Failure> {
    let request: RevokeRequest = if body.is_empty() {
        RevokeRequest::default()
    } else {
        serde_json::from_slice(&body).map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?
    };
    let accounts = ConnectedAccountRepo::new(&state.pool);
    let account = accounts
        .find_by_email_or_account(&auth.user.email, &provider_id)
        .await
        .map_err(internal)?
        .ok_or((StatusCode::NOT_FOUND, "account not found".to_string()))?;
    if request.token.as_deref().is_some_and(|token| token != account.access_token && Some(token) != account.refresh_token.as_deref()) {
        return Err((StatusCode::FORBIDDEN, "token does not belong to the account".to_string()));
    }
    revoke_at_provider(&state, &provider_id, stored_token(&account)).await.map_err(|e| (StatusCode::BAD_GATEWAY, e))?;
    accounts.disconnect(&account.account_id, &provider_id).await.map_err(internal)?;
    SessionRepo::new(&state.pool).revoke(auth.session.id).await.map_err(internal)?;
    Ok((StatusCode::NO_CONTENT, [(header::SET_COOKIE, clear_session_cookie(&state))]).into_response())
}

/// The token to revoke: revoking a refresh token also invalidates the access tokens issued from it.
fn stored_token(account: &ConnectedAccount) -> &str {
    account.refresh_token.as_deref().unwrap_or(&account.access_token)
}

/// RFC 7009 revocation; a provider without a `revoke_url` is skipped.
async fn revoke_at_provider(state: &MeghAuthState, provider_id: &str, token: &str) -> Result<(), String> {
    let Some(url) = state.providers.get(provider_id).and_then(|provider| provider.revoke_url.as_deref()) else {
        return Ok(());
    };
    let response = state.http_client.post(url).form(&[("token", token)]).send().await.map_err(|e| e.to_string())?;
    response.status().is_success().then_some(()).ok_or_else(|| format!("revoke endpoint returned {}", response.status()))
}
