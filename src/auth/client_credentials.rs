//! The OAuth2 client credentials grant (RFC 6749 §4.4): a machine trades its client id and secret for an access token.

use axum::extract::rejection::FormRejection;
use axum::extract::State;
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Form, Json, Router};
use axum_extra::headers::{authorization::Basic, Authorization};
use axum_extra::typed_header::TypedHeaderRejection;
use axum_extra::TypedHeader;
use oauth2::basic::{BasicErrorResponseType, BasicTokenResponse, BasicTokenType};
use oauth2::{AccessToken, EmptyExtraTokenFields, Scope, StandardErrorResponse};
use serde::Deserialize;
use sqlx::PgPool;

use super::grant::Grant;
use super::http::MeghAuthState;
use super::token::JwtSession;
use super::user::{PasswordError, UserRepo, MACHINE};
use crate::tenant::{Member, Tenants};

#[derive(Deserialize)]
struct TokenRequest {
    grant_type: String,
    scope: Option<String>,
}

/// `POST /auth/megh/token`: `grant_type=client_credentials` and an optional `scope` (it only narrows the machine's
/// grants), the client authenticated by `Authorization: Basic`. Answers with the access token or an RFC 6749 §5.2 error.
pub fn token_router(state: MeghAuthState, jwt: JwtSession) -> Router {
    Router::new().route("/auth/megh/token", post(token)).with_state((state.pool, jwt))
}

async fn token(
    State((pool, jwt)): State<(PgPool, JwtSession)>,
    credentials: Result<TypedHeader<Authorization<Basic>>, TypedHeaderRejection>,
    request: Result<Form<TokenRequest>, FormRejection>,
) -> Result<Response, TokenError> {
    let TypedHeader(Authorization(basic)) = credentials.map_err(|_| TokenError::CLIENT)?;
    let member = authenticate(&pool, basic.username(), basic.password()).await?;
    let Form(request) = request.map_err(|_| TokenError::bad(BasicErrorResponseType::InvalidRequest))?;
    (request.grant_type == "client_credentials").then_some(()).ok_or(TokenError::bad(BasicErrorResponseType::UnsupportedGrantType))?;
    let requested: Vec<Grant> = request.scope.unwrap_or_default().split_whitespace().map(Grant::new).collect();
    requested.iter().all(|grant| member.has_grant(grant)).then_some(()).ok_or(TokenError::bad(BasicErrorResponseType::InvalidScope))?;
    let issued = jwt.issue(&member, &requested).map_err(internal)?;
    let mut body = BasicTokenResponse::new(AccessToken::new(issued.jwt), BasicTokenType::Bearer, EmptyExtraTokenFields {});
    body.set_expires_in(Some(&issued.expires_in));
    body.set_scopes(Some(issued.scope.iter().map(|grant| Scope::new(grant.as_str().into())).collect()));
    Ok(([(header::CACHE_CONTROL, "no-store"), (header::PRAGMA, "no-cache")], Json(body)).into_response())
}

/// The membership of the machine whose client id and secret these are.
async fn authenticate(pool: &PgPool, client_id: &str, secret: &str) -> Result<Member, TokenError> {
    let user = UserRepo::new(pool).verify_password(client_id, secret).await.map_err(|error| match error {
        PasswordError::UserNotFound | PasswordError::NoPassword | PasswordError::WrongPassword => TokenError::CLIENT,
        other => internal(other),
    })?;
    (user.data.provider.as_deref() == Some(MACHINE)).then_some(()).ok_or(TokenError::CLIENT)?;
    let memberships = Tenants::new(pool.clone()).memberships(user.id).await.map_err(internal)?;
    memberships.into_iter().next().ok_or(TokenError::CLIENT)
}

/// An RFC 6749 §5.2 error: the status and the error type.
struct TokenError(StatusCode, BasicErrorResponseType);

impl TokenError {
    const CLIENT: Self = Self(StatusCode::UNAUTHORIZED, BasicErrorResponseType::InvalidClient);

    fn bad(error: BasicErrorResponseType) -> Self {
        Self(StatusCode::BAD_REQUEST, error)
    }
}

fn internal(cause: impl std::fmt::Display) -> TokenError {
    tracing::error!("{cause}");
    TokenError(StatusCode::INTERNAL_SERVER_ERROR, BasicErrorResponseType::Extension("server_error".into()))
}

impl IntoResponse for TokenError {
    fn into_response(self) -> Response {
        let mut response = (self.0, Json(StandardErrorResponse::new(self.1, None, None))).into_response();
        if self.0 == StatusCode::UNAUTHORIZED {
            response.headers_mut().insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Basic"));
        }
        response
    }
}
