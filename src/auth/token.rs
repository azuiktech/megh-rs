//! Short-lived JWT access tokens (RFC 9068, the claims megh-go uses) that answer requests without a database
//! lookup. The token comes from `Authorization: Bearer` or, failing that, the cookie. A cookie token that is missing
//! or has expired is renewed from the session, so it is a cache in front of the session; a bearer token is never
//! renewed. Revocation is the token's lifetime: ending the session stops renewal, not the current token.

use std::time::Duration;

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::Response;
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use axum_extra::headers::{authorization::Bearer, Authorization, HeaderMapExt};
use jsonwebtoken::errors::ErrorKind;
use jsonwebtoken::{decode, encode, get_current_timestamp, Algorithm, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use tower::{Layer, ServiceExt};
use tower_http::csrf::CsrfLayer;
use tower_sessions::Session;
use uuid::Uuid;

use super::http::USER_ID;
use super::Grant;
use crate::org::{Member, Orgs};

pub const JWT_COOKIE: &str = "jwt_token";
const ISSUER: &str = "megh";

/// `sub` is the member id, `aud` the organization id and `scope` the member's grants, space separated.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccessToken {
    pub sub: Uuid,
    pub iss: String,
    pub aud: Vec<String>,
    pub exp: u64,
    pub iat: u64,
    #[serde(default)]
    pub client_id: String,
    #[serde(default)]
    pub scope: String,
}

impl AccessToken {
    pub fn new(member: &Member, ttl: Duration) -> Self {
        let now = get_current_timestamp();
        Self {
            sub: member.id,
            iss: ISSUER.into(),
            aud: vec![member.organization_id.to_string()],
            exp: now + ttl.as_secs(),
            iat: now,
            client_id: ISSUER.into(),
            scope: member.grants.join(" "),
        }
    }

    pub fn grants(&self) -> Vec<Grant> {
        self.scope.split_whitespace().map(Grant::new).collect()
    }
}

/// A signed access token with the lifetime and grants it carries. No `Debug`: the token is a secret.
pub struct IssuedToken {
    pub jwt: String,
    pub expires_in: Duration,
    pub scope: Vec<Grant>,
}

/// Signing key, token lifetime and the pool the renewal reads memberships from.
#[derive(Clone)]
pub struct JwtSession {
    encoding: EncodingKey,
    decoding: DecodingKey,
    validation: Validation,
    ttl: Duration,
    orgs: Orgs,
}

impl JwtSession {
    pub fn new(secret: &[u8], ttl: Duration, pool: PgPool) -> Self {
        let mut validation = Validation::new(Algorithm::HS256);
        validation.set_issuer(&[ISSUER]);
        validation.validate_aud = false;
        validation.leeway = 0;
        Self { encoding: EncodingKey::from_secret(secret), decoding: DecodingKey::from_secret(secret), validation, ttl, orgs: Orgs::new(pool) }
    }

    fn decode(&self, jwt: &str) -> Result<AccessToken, jsonwebtoken::errors::Error> {
        decode::<AccessToken>(jwt, &self.decoding, &self.validation).map(|token| token.claims)
    }

    /// A token for `member`; a non-empty `requested` narrows its grants to those given.
    pub fn issue(&self, member: &Member, requested: &[Grant]) -> Result<IssuedToken, jsonwebtoken::errors::Error> {
        let mut token = AccessToken::new(member, self.ttl);
        if !requested.is_empty() {
            token.scope = requested.iter().map(Grant::as_str).collect::<Vec<_>>().join(" ");
        }
        let jwt = encode(&Header::default(), &token, &self.encoding)?;
        Ok(IssuedToken { jwt, expires_in: self.ttl, scope: token.grants() })
    }

    /// A new token for the user's first membership, taken from the session.
    async fn renew(&self, jar: CookieJar, session: &Session) -> Result<(CookieJar, Vec<Grant>), StatusCode> {
        let user_id: Uuid = session.get(USER_ID).await.map_err(internal)?.ok_or(StatusCode::UNAUTHORIZED)?;
        let member = self.orgs.memberships(user_id).await.map_err(internal)?.into_iter().next().ok_or(StatusCode::UNAUTHORIZED)?;
        let issued = self.issue(&member, &[]).map_err(internal)?;
        Ok((jar.add(cookie(issued.jwt, self.ttl)), issued.scope))
    }
}

/// Puts the grants of a valid token into the request for `authorizer`. A bearer token must be valid; a missing or
/// expired cookie token is renewed from the session. A token that fails any other check is rejected. Mount it inside
/// the session layer.
pub async fn jwt_session(
    State(jwt): State<JwtSession>,
    jar: CookieJar,
    session: Session,
    mut request: Request,
    next: Next,
) -> Result<(CookieJar, Response), StatusCode> {
    let (jar, grants) = match request.headers().typed_get::<Authorization<Bearer>>() {
        Some(Authorization(bearer)) => (jar, jwt.decode(bearer.token()).map_err(|_| StatusCode::UNAUTHORIZED)?.grants()),
        None => from_cookie(&jwt, jar, &session).await?,
    };
    request.extensions_mut().insert(grants);
    Ok((jar, next.run(request).await))
}

/// The CSRF check for requests that sign in by cookie. A bearer token is not sent by a browser on its own, so a request
/// that carries one is not checked; any other goes through `csrf`. That is safe because `jwt_session` then uses the
/// bearer token alone and never the cookie. Use it as `from_fn_with_state(CsrfLayer::new(), csrf_unless_bearer)`.
pub async fn csrf_unless_bearer(State(csrf): State<CsrfLayer>, request: Request, next: Next) -> Response {
    if request.headers().typed_get::<Authorization<Bearer>>().is_some() {
        return next.run(request).await;
    }
    let Ok(response) = csrf.layer(next).oneshot(request).await;
    response.map(Body::new)
}

async fn from_cookie(jwt: &JwtSession, jar: CookieJar, session: &Session) -> Result<(CookieJar, Vec<Grant>), StatusCode> {
    match jar.get(JWT_COOKIE).map(|cookie| jwt.decode(cookie.value())) {
        Some(Ok(token)) => Ok((jar, token.grants())),
        Some(Err(error)) if !matches!(error.kind(), ErrorKind::ExpiredSignature) => Err(StatusCode::UNAUTHORIZED),
        _ => jwt.renew(jar, session).await,
    }
}

fn cookie(token: String, ttl: Duration) -> Cookie<'static> {
    Cookie::build((JWT_COOKIE, token))
        .http_only(true)
        .secure(true)
        .same_site(SameSite::Lax)
        .path("/")
        .max_age(ttl.try_into().expect("a token lifetime fits a cookie's max age"))
        .build()
}

fn internal(_: impl std::error::Error) -> StatusCode {
    StatusCode::INTERNAL_SERVER_ERROR
}
