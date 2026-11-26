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
use crate::Db;
use tower::{Layer, ServiceExt};
use tower_http::csrf::CsrfLayer;
use tower_sessions::Session;
use uuid::Uuid;

use super::http::USER_ID;
use super::{AccessToken, Grant};
use crate::messaging::{CLIENT, current};
use crate::tenant::{Member, Tenants};

pub const JWT_COOKIE: &str = "jwt_token";
const ISSUER: &str = "megh";

impl AccessToken {
    pub fn new(member: &Member, ttl: Duration) -> Self {
        let now = get_current_timestamp();
        Self {
            sub: member.id,
            iss: ISSUER.into(),
            aud: vec![member.tenant_id.to_string()],
            exp: now + ttl.as_secs(),
            iat: now,
            client_id: ISSUER.into(),
            scope: member.grants.join(" "),
        }
    }
}

/// A signed access token with the lifetime and grants it carries. No `Debug`: the token is a secret.
pub struct IssuedToken {
    pub jwt: String,
    pub expires_in: Duration,
    pub scope: Vec<Grant>,
}

/// What a client's token may do: only the client routes of messaging.
const CLIENT_GRANTS: [&str; 5] = ["members:read", "messages:create", "events:create", "attachments:create", "attachments:read"];

/// Signing key, token lifetime and the pool the renewal reads memberships and clients from.
#[derive(Clone)]
pub struct JwtSession {
    encoding: EncodingKey,
    decoding: DecodingKey,
    validation: Validation,
    ttl: Duration,
    tenants: Tenants,
    pool: Db,
}

impl JwtSession {
    pub fn new(secret: &[u8], ttl: Duration, pool: Db) -> Self {
        let mut validation = Validation::new(Algorithm::HS256);
        validation.set_issuer(&[ISSUER]);
        validation.validate_aud = false;
        validation.leeway = 0;
        Self { encoding: EncodingKey::from_secret(secret), decoding: DecodingKey::from_secret(secret), validation, ttl, tenants: Tenants::new(pool.clone()), pool }
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

    /// A token for a client of the tenant, carrying only the client routes' grants.
    pub fn issue_client(&self, tenant_id: Uuid, client_id: Uuid) -> Result<IssuedToken, jsonwebtoken::errors::Error> {
        let now = get_current_timestamp();
        let scope = [format!("tenants:read:{tenant_id}")].into_iter().chain(CLIENT_GRANTS.map(str::to_string)).collect::<Vec<_>>().join(" ");
        let tenant = tenant_id.to_string();
        let token = AccessToken { sub: client_id, iss: ISSUER.into(), aud: vec![tenant], exp: now + self.ttl.as_secs(), iat: now, client_id: ISSUER.into(), scope };
        let jwt = encode(&Header::default(), &token, &self.encoding)?;
        Ok(IssuedToken { jwt, expires_in: self.ttl, scope: token.grants() })
    }

    /// The jar with the client's token cookie added.
    pub(crate) fn client_cookie(&self, jar: CookieJar, tenant_id: Uuid, client_id: Uuid) -> Result<CookieJar, jsonwebtoken::errors::Error> {
        self.issue_client(tenant_id, client_id).map(|issued| jar.add(cookie(issued.jwt, self.ttl)))
    }

    /// A new token from the session: the user's first membership, or else the session's client (or the client it
    /// moved to).
    async fn renew(&self, jar: CookieJar, session: &Session) -> Result<(CookieJar, AccessToken), StatusCode> {
        let issued = match session.get::<Uuid>(USER_ID).await.map_err(internal)? {
            Some(user_id) => self.member_token(user_id).await?,
            None => self.client_token(session).await?,
        };
        let token = self.decode(&issued.jwt).map_err(internal)?;
        Ok((jar.add(cookie(issued.jwt, self.ttl)), token))
    }

    async fn member_token(&self, user_id: Uuid) -> Result<IssuedToken, StatusCode> {
        let member = self.tenants.memberships(user_id).await.map_err(internal)?.into_iter().next().ok_or(StatusCode::UNAUTHORIZED)?;
        self.issue(&member, &[]).map_err(internal)
    }

    async fn client_token(&self, session: &Session) -> Result<IssuedToken, StatusCode> {
        let (tenant_id, kept): (Uuid, Uuid) = session.get(CLIENT).await.map_err(internal)?.ok_or(StatusCode::UNAUTHORIZED)?;
        let client_id = current(&self.pool, tenant_id, kept).await.map_err(internal)?.ok_or(StatusCode::UNAUTHORIZED)?;
        session.insert(CLIENT, (tenant_id, client_id)).await.map_err(internal)?;
        self.issue_client(tenant_id, client_id).map_err(internal)
    }
}

/// Puts a valid token and its grants into the request for `authorizer`. A bearer token must be valid; a missing or
/// expired cookie token is renewed from the session. A token that fails any other check is rejected. Mount it inside
/// the session layer.
pub async fn jwt_session(
    State(jwt): State<JwtSession>,
    jar: CookieJar,
    session: Session,
    mut request: Request,
    next: Next,
) -> Result<(CookieJar, Response), StatusCode> {
    let (jar, token) = match request.headers().typed_get::<Authorization<Bearer>>() {
        Some(Authorization(bearer)) => (jar, jwt.decode(bearer.token()).map_err(|_| StatusCode::UNAUTHORIZED)?),
        None => from_cookie(&jwt, jar, &session).await?,
    };
    request.extensions_mut().insert(token.grants());
    request.extensions_mut().insert(token);
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

async fn from_cookie(jwt: &JwtSession, jar: CookieJar, session: &Session) -> Result<(CookieJar, AccessToken), StatusCode> {
    match jar.get(JWT_COOKIE).map(|cookie| jwt.decode(cookie.value())) {
        Some(Ok(token)) => Ok((jar, token)),
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
