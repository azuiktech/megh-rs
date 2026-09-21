//! Short-lived JWT access tokens (RFC 9068, the claims megh-go uses) that answer requests without a database
//! lookup. When the token is missing or has expired it is renewed from the session, so the token is a cache in
//! front of the session. Revocation is the token's lifetime: ending the session stops renewal, not the current token.

use std::time::Duration;

use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::middleware::Next;
use axum::response::Response;
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use jsonwebtoken::errors::ErrorKind;
use jsonwebtoken::{decode, encode, get_current_timestamp, Algorithm, DecodingKey, EncodingKey, Header, Validation};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
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

    /// A new token for the user's first membership, taken from the session.
    async fn renew(&self, jar: CookieJar, session: &Session) -> Result<(CookieJar, Vec<Grant>), StatusCode> {
        let user_id: Uuid = session.get(USER_ID).await.map_err(internal)?.ok_or(StatusCode::UNAUTHORIZED)?;
        let member = self.orgs.memberships(user_id).await.map_err(internal)?.into_iter().next().ok_or(StatusCode::UNAUTHORIZED)?;
        let token = AccessToken::new(&member, self.ttl);
        let jwt = encode(&Header::default(), &token, &self.encoding).map_err(internal)?;
        Ok((jar.add(cookie(jwt, self.ttl)), token.grants()))
    }
}

/// Puts the grants of a valid token into the request for `authorizer`; renews a missing or expired token from the
/// session. A token that fails any other check is rejected. Mount it inside the session layer.
pub async fn jwt_session(
    State(jwt): State<JwtSession>,
    jar: CookieJar,
    session: Session,
    mut request: Request,
    next: Next,
) -> Result<(CookieJar, Response), StatusCode> {
    let token = jar.get(JWT_COOKIE).map(|cookie| decode::<AccessToken>(cookie.value(), &jwt.decoding, &jwt.validation));
    let (jar, grants) = match token {
        Some(Ok(token)) => (jar, token.claims.grants()),
        Some(Err(error)) if !matches!(error.kind(), ErrorKind::ExpiredSignature) => return Err(StatusCode::UNAUTHORIZED),
        _ => jwt.renew(jar, &session).await?,
    };
    request.extensions_mut().insert(grants);
    Ok((jar, next.run(request).await))
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
