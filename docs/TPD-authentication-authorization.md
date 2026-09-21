# TPD — Authentication & Authorization

**Status:** F1–F7 shipped. F8 (OAuth callback hardening, azuiktech/megh-rs#22) is in scope and awaiting approval of §7.3.
**Modules:** `src/auth`, `src/session`, `src/account`, `src/org`, `ui/sdk/src/auth.ts`, `migrations/0001–0004`.
**Depends on:** `Entity<ID, T>` (`src/entity.rs`) for `User`, `Session` and `SessionView`.

This is the living design for everything that answers "who is calling" (authentication) and "may they do this" (authorization). Change-level detail belongs in `CHANGELOG.md`; this document changes when the feature's shape changes.

## 1. Delivery

| # | Feature | Status | PR / issue |
|---|---|---|---|
| F1 | Org tenancy and Shiro-style permission grants | `[x]` | #2 / #1 |
| F2 | User identity (`users`, `UserRepo`) | `[x]` | #4 / #3 |
| F3 | OAuth 2.0 client and connected accounts | `[x]` | #6 / #5 |
| F4 | Sessions; `User` as `Entity` | `[x]` | #8 / #7 |
| F5 | Axum auth router, session cookie, `AuthUser` extractor | `[x]` | #10 / #9 |
| F6 | OAuth popup `postMessage` protocol, `ui/sdk`, configurable redirect URI | `[x]` | #11 |
| F7 | Route-based grant authorizer middleware | `[x]` | #20 / #19 |
| F8 | OAuth callback hardening (state, PKCE, `Secure`, verified email, `postMessage` origin) | `[~]` design pending approval, branch `fix/oauth-callback-hardening` | #22 |

## 2. Design vocabulary (pac4j)

Terms follow pac4j (https://www.pac4j.org/docs/).

| pac4j | Contract | In megh |
|---|---|---|
| Authenticator / client | turns a request into a verified user | OAuth login and callback (F3, F5) |
| Matcher | does security apply to this request? | Axum routing and `route_layer`. No `Matcher` trait. |
| Authorizer | given a request (and profile), allow or deny; deny is 403 | `authorizer` middleware over `Grant`s (F7). F8 adds a callback-level `Authorizer` trait. |

pac4j's `csrfCheck` is an app-level double-submit check on POSTs. Its OAuth clients keep `state` and PKCE inside the client.

## 3. Architecture

- **Stack:** axum 0.8, sqlx 0.8 (Postgres), `oauth2` 4.4 (reqwest, rustls), `sha2`/`hex` for token hashing, `reqwest` for userinfo.
- **Feature flags:** `postgres` gates repos and `sqlx::FromRow`; `client` gates `reqwest` and `fetch_user_info`; `axum` gates the router, extractor and authorizer middleware. The router (`auth::http`) needs both `axum` and `postgres`. All three are default.
- **Authentication path:** browser → `/auth/{provider}/login` → provider → `/auth/{provider}/callback` → code exchange → userinfo → `users` upsert → `connected_accounts` upsert → `sessions` insert → session cookie. Later requests: cookie → `SessionRepo::find_valid_by_token` → `UserRepo::get_by_id` → `AuthUser`.
- **Authorization path:** the application puts an `OrgMember` (or a `Vec<Grant>`) into request extensions; the `authorizer` middleware derives the required `Grant` from the matched route and method and checks it. megh does not populate those extensions; nothing links `AuthUser` to `OrgMember` yet (see §8).

## 4. Data model

| Table | Columns | Constraints |
|---|---|---|
| `org` | `id`, `name`, `description`, `timezone`, `created_at` | PK `id` |
| `org_members` | `id`, `organization_id`, `user_id`, `joined_at`, `grants TEXT[]` | FK `organization_id` → `org` (cascade); unique `(organization_id, user_id)`; `user_id` has no FK |
| `users` | `id`, `subject`, `email`, `display_name`, `photo_url`, `created_at`, `updated_at` | unique `subject`, unique `email` |
| `connected_accounts` | `account_id`, `provider`, `email`, `access_token`, `refresh_token`, `token_type`, `expiry`, `created_at`, `updated_at`, `disconnected_at` | PK `(account_id, provider)`; no `user_id` |
| `sessions` | `id`, `user_id`, `token_hash`, `expires_at`, `user_agent`, `ip_address`, `metadata JSONB`, `created_at`, `updated_at` | FK `user_id` → `users` (cascade); unique `token_hash` |

`megh::migrate(&pool)` runs `0001`–`0004`; `examples/server.rs` uses `sqlx::migrate!`.

## 5. Implemented features and their APIs

### F1 — Org tenancy and grants (`org`, `auth::grant`)

```rust
pub struct Org { id: Uuid, name: String, description: String, timezone: String, created_at: DateTime<Utc> }
pub struct OrgMember { id, organization_id, user_id: Uuid, joined_at: DateTime<Utc>, grants: Vec<String> }
impl OrgMember { pub fn has_grant(&self, required: &Grant) -> bool }

pub struct Grant(pub String);                 // "resource:action:instance"
impl Grant {
    pub fn new(permission: impl Into<String>) -> Self;                       // trims and lowercases
    pub fn from_parts(resource: &str, action: &str, instance: Option<&str>) -> Self;
    pub fn as_str(&self) -> &str;
    pub fn resource(&self) -> &str;
    pub fn action(&self) -> Option<&str>;
    pub fn instance(&self) -> Option<&str>;
    pub fn implies(&self, target: &Grant) -> bool;
}
```

`implies` rules (Apache Shiro): `*` implies everything; `*` matches any part; commas list alternatives (`course:read,write`); a grant with fewer parts implies deeper targets (`course:read` implies `course:read:123`); a grant with more parts implies a shorter target only if the extra parts are `*`. Grants are lowercased in full, including the instance part.

### F2 — User identity (`auth::user`)

```rust
pub type User = Entity<Uuid, UserProfile>;
pub struct UserProfile { subject: String, email: String, display_name: String, photo_url: String }
pub struct UpsertUserInput { subject: String, email: String, display_name: Option<String>, photo_url: Option<String> }

impl UserRepo<'_> {                            // feature "postgres"
    pub fn new(pool: &PgPool) -> UserRepo;
    pub async fn get_by_id(&self, id: Uuid) -> Result<Option<User>, sqlx::Error>;
    pub async fn find_by_email(&self, email: &str) -> Result<Option<User>, sqlx::Error>;
    pub async fn find_by_subject(&self, subject: &str) -> Result<Option<User>, sqlx::Error>;
    pub async fn upsert(&self, input: &UpsertUserInput) -> Result<User, sqlx::Error>;   // conflict key: email
    pub async fn update_profile(&self, id: Uuid, display_name: &str, photo_url: &str) -> Result<User, sqlx::Error>;
}
```

`upsert` overwrites `subject` and keeps existing `display_name`/`photo_url` when the new value is empty. Subjects are stored as `"{provider}:{provider_subject}"`.

### F3 — OAuth 2.0 client and connected accounts (`auth::oauth`, `account`)

```rust
pub struct OAuthProviderConfig {
    provider_id: String, client_id: String, client_secret: Option<String>,
    auth_url: String, token_url: String, userinfo_url: Option<String>,
    default_scopes: Vec<String>, redirect_url: Option<String>,
}
impl OAuthProviderConfig {
    pub fn google(client_id: impl Into<String>, client_secret: Option<String>) -> Self;
    pub fn with_redirect_url(self, redirect_url: impl Into<String>) -> Self;
    pub fn build_client(&self, redirect_url: Option<RedirectUrl>) -> Result<BasicClient, OAuthError>;
}

pub struct AuthUrlOptions<'a> {
    scopes: &'a [&'a str], pkce: Option<&'a PkceCodeChallenge>,
    offline_access: bool,     // access_type=offline
    incremental: bool,        // include_granted_scopes=true
    prompt: Option<&'a str>,
}
pub fn build_authorization_url(client: &BasicClient, csrf_token: CsrfToken, opts: AuthUrlOptions) -> Url;

pub struct OAuthUserInfo { subject: String, email: String, email_verified: Option<bool>, name: Option<String>, picture: Option<String> }
pub async fn fetch_user_info(http: &reqwest::Client, userinfo_url: &str, access_token: &str)
    -> Result<OAuthUserInfo, OAuthError>;               // feature "client"; subject = `sub` or `id`

pub enum OAuthFlowMode { WebRedirect{..}, DesktopLoopback{..}, CustomScheme{..} }   // each { redirect_uri: String }
pub enum OAuthError { UrlParse, Http, TokenExchange, InvalidResponse }

pub struct ConnectedAccount { account_id, provider, email, access_token, refresh_token, token_type, expiry, created_at, updated_at, disconnected_at }
impl ConnectedAccount { pub fn is_connected(&self) -> bool; pub fn is_expired(&self, margin_secs: i64) -> bool }
pub struct OAuth2Tokens { access_token, refresh_token, token_expires_at }      // From<&BasicTokenResponse>

impl ConnectedAccountRepo<'_> {                        // feature "postgres"
    pub fn new(pool: &PgPool) -> ConnectedAccountRepo;
    pub async fn get(&self, account_id: &str, provider: &str) -> Result<Option<ConnectedAccount>, sqlx::Error>;
    pub async fn find_by_email_or_account(&self, identifier: &str, provider: &str) -> Result<Option<ConnectedAccount>, sqlx::Error>;
    pub async fn save(&self, account: &ConnectedAccount) -> Result<ConnectedAccount, sqlx::Error>;   // upsert
    pub async fn disconnect(&self, account_id: &str, provider: &str) -> Result<bool, sqlx::Error>;   // soft
    pub async fn list_by_email(&self, email: &str) -> Result<Vec<ConnectedAccount>, sqlx::Error>;
}
```

`oauth2` types (`BasicClient`, `CsrfToken`, `PkceCodeChallenge`, `PkceCodeVerifier`, `RedirectUrl`, `Scope`, `TokenResponse`, …) are re-exported from `megh::auth` and `megh::`. Provider factories exist only for Google; other providers are built with the generic config.

### F4 — Sessions (`session`)

```rust
pub type Session = Entity<Uuid, FullSession>;          // FullSession = SessionData + SessionSecrets { token_hash }
pub type SessionView = Entity<Uuid, SessionData>;      // no token hash; safe to serialize
pub struct SessionData { user_id: Uuid, expires_at: DateTime<Utc>, user_agent: String, ip_address: String, metadata: serde_json::Value }
impl SessionData { pub fn is_expired(&self) -> bool }
pub struct CreatedSession { session: Session, plaintext_token: String }          // token is returned once
pub trait SessionExt { fn to_view(&self) -> SessionView }

pub fn generate_session_token() -> String;             // two UUIDv4 (simple), joined by '-'
pub fn hash_session_token(token: &str) -> String;      // SHA-256, hex

impl SessionRepo<'_> {                                 // feature "postgres"
    pub fn new(pool: &PgPool) -> SessionRepo;
    pub async fn create(&self, user_id: Uuid, duration: Duration, user_agent: &str, ip_address: &str, metadata: serde_json::Value)
        -> Result<CreatedSession, sqlx::Error>;
    pub async fn find_valid_by_token(&self, plaintext_token: &str) -> Result<Option<Session>, sqlx::Error>;   // hash lookup, expires_at > NOW()
    pub async fn touch(&self, id: Uuid, extension: Duration) -> Result<Option<Session>, sqlx::Error>;         // adds to expires_at
    pub async fn revoke(&self, id: Uuid) -> Result<bool, sqlx::Error>;
    pub async fn revoke_by_token(&self, plaintext_token: &str) -> Result<bool, sqlx::Error>;
    pub async fn revoke_all_for_user(&self, user_id: Uuid) -> Result<u64, sqlx::Error>;
    pub async fn list_by_user(&self, user_id: Uuid) -> Result<Vec<Session>, sqlx::Error>;
}
```

Only the SHA-256 hash is stored; revocation deletes the row.

### F5 — Axum auth router (`auth::http`, features `axum` + `postgres`)

```rust
pub struct MeghAuthState {                              // Clone; all fields pub
    pool: PgPool, cookie_name: String, session_duration: Duration, redirect_after_login: String,
    app_origin: String, providers: Arc<HashMap<String, OAuthProviderConfig>>, http_client: reqwest::Client,
}
impl MeghAuthState {
    pub fn new(pool: PgPool) -> Self;                   // cookie "kyrios_session", 30 days, redirect "/", origin "http://localhost:8080"
    pub fn with_cookie_name(self, impl Into<String>) -> Self;
    pub fn with_redirect_after_login(self, impl Into<String>) -> Self;
    pub fn with_app_origin(self, impl Into<String>) -> Self;
    pub fn add_provider(self, OAuthProviderConfig) -> Self;
}
pub fn auth_router(state: MeghAuthState) -> Router;
pub struct AuthUser { pub user: User, pub session: Session }   // FromRequestParts<S> where MeghAuthState: FromRef<S>
pub struct AuthMeResponse { pub user: User, pub session: SessionView }
pub use axum::extract::FromRef;                         // replaces the former hand-written trait
pub fn extract_cookie(headers: &HeaderMap, name: &str) -> Option<String>;
```

| Route | Behaviour |
|---|---|
| `GET /auth/{provider}`, `GET /auth/{provider}/login` | 303 to the provider authorization URL (`access_type=offline`, `include_granted_scopes=true`, `prompt=select_account`, provider default scopes). 404 unknown provider; 500 bad callback URL. |
| `GET /auth/{provider}/callback`, `GET /auth/{provider}/token` | Same handler. Query: `code`, `state`, `error`, `error_description`. Exchanges the code, fetches userinfo, upserts the user, saves the `connected_accounts` row, creates a session. 200 HTML with `Set-Cookie`. 400 provider `error` or missing `code`; 404 unknown provider; 502 exchange or userinfo failure; 500 database failure. Callback URL defaults to `{app_origin}/auth/{provider}/token` unless `redirect_url` is set. |
| `GET /auth/me` | 200 `AuthMeResponse`. 401 missing cookie, unknown or expired session, or missing user; 500 database error. |
| `POST /auth/logout` | Revokes the session if the cookie is present, clears the cookie, always 200 `{"status":"ok"}`. |

Session cookie: `{cookie_name}={token}; Path=/; HttpOnly; SameSite=Lax; Max-Age={session_duration}`.

### F6 — Popup protocol and UI SDK (`ui/sdk/src/auth.ts`)

The callback returns an HTML page that runs `window.opener.postMessage({type:"oauth_success", user}, "*")` and closes; without an opener it navigates to `redirect_after_login`.

`Auth<User>` (TypeScript class), backed by routes that exist:

- `restore()` calls `GET /auth/me` and sets the user (null on failure).
- `signInWithOAuth(provider, mode = 'auto')` opens `/auth/{provider}/login` in a popup, or redirects (modes `popup`, `redirect`, `auto`). It accepts `oauth_success` only from the backend origin.
- `signInWithGoogle(mode)`, `signOut()` (`POST /auth/logout`), `onChange(fn)`, `user`.
- Configurable base URL, route paths and response `transform`.

The SDK also defines `signInWithPassword`, `connectWithOAuth`/`connectWithGoogle`, `disconnectWithOAuth`/`disconnectWithGoogle` and `revokeOAuth`/`revokeGoogle`. The server has no matching routes (§8).

### F7 — Route-based authorizer (`auth::authorizer`, feature `axum`)

```rust
pub fn request_action(method: &str) -> &'static str;   // POST→create, PUT/PATCH→update, DELETE→delete, else read
pub fn request_grant(method: &str, matched_template: Option<&str>, uri_path: &str) -> Grant;
pub async fn authorizer(req: Request, next: Next) -> Result<Response, (StatusCode, &'static str)>;
```

`authorizer` is an Axum `from_fn` middleware. It reads an `OrgMember` (else a `Vec<Grant>`) from request extensions and requires a `MatchedPath`. The requested grant is `resource:action[:instance]`: `resource` is the last static path segment, or the segment before the last path parameter; `instance` is the last path parameter's value.

Responses: 401 `not authenticated` (no `OrgMember` or grants in extensions), 403 `not permitted`, 404 `route not found` (no matched path). Verified by `tests/authorizer_test.rs` and `tests/course_authorizer_test.rs` with a fake injector.

## 6. Test coverage (shipped)

`tests/grant_test.rs`, `authorizer_test.rs`, `course_authorizer_test.rs` (F1, F7); `session_user_test.rs` (F2, F4); `account_oauth_test.rs` (F3); `auth_router_test.rs` (F5); unit tests in `auth/grant.rs`, `auth/user.rs`, `session/*`, `org/member.rs`. Repo and router tests use a lazy pool and never query. The exception is `test_connected_account_repo_persistence`, which uses `DATABASE_URL` if reachable and otherwise returns early and passes without asserting anything. No shipped test covers `UserRepo`, `SessionRepo` or the callback against a live database.

## 7. F8 — OAuth callback hardening (in scope, pending)

**Issue:** azuiktech/megh-rs#22
**Touches:** `src/auth/http.rs` (edit), `src/auth/flow.rs` (new), `src/auth/mod.rs` and `src/lib.rs` (`mod` + re-export lines), `tests/auth_router_test.rs` (extend). 5 files.

### 7.1 Problem

`oauth_login` builds a `CsrfToken` and drops it; `oauth_callback` never checks `state`, sends no PKCE verifier, sets the session cookie without `Secure`, links accounts by email without checking `email_verified`, and posts the user object with `targetOrigin "*"`.

### 7.2 Design

Standard OAuth 2.0 authorization-code + PKCE with the `oauth2` crate's `CsrfToken` and `PkceCodeChallenge::new_random_sha256()`. No hand-rolled crypto, no signing key. In pac4j terms, the OAuth `state` check is an authorizer on the callback and PKCE stays in the authenticator, because it is part of the token exchange.

```
GET /auth/{provider}/login
  csrf = CsrfToken::new_random();  (challenge, verifier) = PkceCodeChallenge::new_random_sha256()
  Set-Cookie: _oauth_state_<provider>=<csrf>       HttpOnly; SameSite=Lax; Path=/; Max-Age=600; [Secure]
  Set-Cookie: _oauth_pkce_<provider>=<verifier>    (same attributes)
  303 → provider auth URL with state=<csrf>, code_challenge=<challenge>, code_challenge_method=S256

GET /auth/{provider}/callback?code&state
  run every configured Authorizer          any Err → 403
  verifier = cookie _oauth_pkce_<provider>              else 400
  Clear both cookies (Max-Age=0), success or failure
  exchange_code(code).set_pkce_verifier(verifier)
  userinfo.email_verified == Some(true)                 else 403
  upsert user → create session → Set-Cookie session [Secure when app_origin is https]
  HTML: postMessage(payload, <web origin>)
```

Why a plain cookie compare and not a signed state: the state cookie is HttpOnly and `SameSite=Lax`, so a cross-site attacker cannot set or read it in the victim's browser; comparing it with the query `state` is the standard double-submit check and needs no secret. (megh-go signs it with an HMAC keyed off the client secret; that adds nothing here.)

Decisions:

| Point | Choice | Reason |
|---|---|---|
| CSRF is configurable, default on | The callback's authorizer list defaults to `[OAuthState]`. Opt out with `with_authorizers([])`. | Composition instead of a flag: no `bool`, no enum, and any other filter (IP allow-list, custom) plugs in the same way. |
| Deny status | 403 for every authorizer denial | pac4j: authorization failure = 403. Missing PKCE verifier is a malformed request, 400. |
| `Secure` flag | Derived: `app_origin` starts with `https://`. | Local `http://127.0.0.1` dev must keep working. |
| `email_verified` | Require `Some(true)` | `upsert` links by email; `None` is not proof. |
| `postMessage` target | Origin of `redirect_after_login` resolved against `app_origin` (`Url::join(..).origin()`) | `app_origin` is the API origin; the opener is the web app. |
| Cookie names | `_oauth_state_<provider>`, `_oauth_pkce_<provider>` | Same as megh-go. |

Login always sets the state cookie, even with no `OAuthState` authorizer: it is harmless and keeps `login` free of a conditional.

Not changed: session cookie `SameSite=Lax`, userinfo-endpoint identity, `include_granted_scopes`/`prompt=select_account`. The existing grant `authorizer` (F7) is a profile-based authorizer in pac4j terms and is not touched.

### 7.3 Interfaces (for approval)

Public additions (in `src/auth/flow.rs`, `pub mod flow`, re-exported from `megh::auth` and `megh::`). No existing signature changes.

```rust
/// What an authorizer sees of an OAuth callback.
pub struct CallbackRequest<'a> {
    pub provider: &'a str,
    pub headers: &'a HeaderMap,
    pub query: &'a OAuthCallbackQuery,   // existing type, already public
}

/// Why an authorizer refused the request.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct Denied(pub &'static str);

/// A request filter: `Ok` lets the callback proceed, `Err` denies it.
pub trait Authorizer: Send + Sync {
    fn authorize(&self, request: &CallbackRequest) -> Result<(), Denied>;
}

/// Denies unless the `state` query equals the state cookie set at login.
pub struct OAuthState;
impl Authorizer for OAuthState { /* constant-time compare */ }

impl MeghAuthState {
    // new field: authorizers: Arc<[Arc<dyn Authorizer>]>, default [OAuthState]
    pub fn with_authorizers(
        self,
        authorizers: impl IntoIterator<Item = Arc<dyn Authorizer>>,
    ) -> Self;
}
```

Crate-private helpers in `flow.rs` (`pub(super)`):

```rust
struct Attempt { csrf: CsrfToken, challenge: PkceCodeChallenge, cookies: [String; 2] }
fn begin(provider: &str, secure: bool) -> Attempt;
fn pkce_verifier(headers: &HeaderMap, provider: &str) -> Option<PkceCodeVerifier>;
fn clear_cookies(provider: &str) -> [String; 2];
fn require_verified_email(info: &OAuthUserInfo) -> Result<(), Denied>;
```

Changes to `src/auth/http.rs` (inside existing functions, no signature changes):

- `oauth_login`: `flow::begin`, `pkce: Some(&challenge)` in `AuthUrlOptions`, cookies on the redirect.
- `oauth_callback`: run the authorizers first (`state.authorizers.iter().try_for_each(..)`, no raw loop), then `pkce_verifier`, `.set_pkce_verifier(..)` on the exchange, `require_verified_email` after userinfo, append `clear_cookies`; `Denied` → 403 via one `impl From<Denied> for (StatusCode, String)`.
- Session cookie gains `; Secure` from one helper `fn secure(state) -> bool`.
- Popup HTML: `postMessage(d, <origin JSON>)`.

`extract_cookie` (existing, public) reads the attempt cookies.

### 7.4 Tests (written before implementation, then immutable)

In `tests/auth_router_test.rs`; existing tests unchanged.

Router-level (`tower::oneshot`, lazy pool; rejection happens before any DB access):

1. `/auth/google/login` → 303; `Location` has `state=`, `code_challenge=`, `code_challenge_method=S256`; `_oauth_state_google` and `_oauth_pkce_google` cookies with `HttpOnly`, `SameSite=Lax`, `Max-Age=600`; state cookie value equals the `state` in `Location`.
2. `https` `app_origin` → attempt cookies carry `Secure`; `http` → they don't.
3. Callback, no cookies → 403.
4. Callback, `state` ≠ cookie → 403; no session cookie set.
5. Callback, matching state, no PKCE cookie → 400.
6. Denials clear both attempt cookies.
7. `MeghAuthState::new(..)` default authorizers = exactly `[OAuthState]` (asserted by behaviour: test 3 passes without configuring anything).
8. `with_authorizers([])` → callback with no state cookie is not denied by a filter (fails later at the exchange, not with 403); missing PKCE is still 400.
9. A custom `Authorizer` that always denies, passed to `with_authorizers`, → 403 even with a valid state.

Unit (`flow.rs`): `OAuthState` accepts equal, rejects a one-byte difference, rejects missing cookie/query; `require_verified_email` accepts only `Some(true)`.

End-to-end (real HTTP, real Postgres, stub provider). A tokio test starts a local stub OAuth server (token endpoint checks `code_verifier` against the challenge it saw at login; userinfo returns a configurable profile), points `OAuthProviderConfig` `auth_url`/`token_url`/`userinfo_url` at it, and drives `auth_router` over a TCP listener with `reqwest` and a cookie jar:

10. Happy path: login → stub → callback → session cookie, `Secure` per origin, `/auth/me` returns the user, user row in Postgres.
11. `email_verified=false` → 403, no user row, no session.
12. Wrong PKCE verifier → 502, no session.
13. Popup HTML has the web origin as `targetOrigin`, not `"*"`.
14. `/auth/logout` → `/auth/me` is 401 afterwards.

Tests 10–14 need `DATABASE_URL` (local Postgres.app). They fail loudly with a clear message when it is unset; they never pass silently. Live-Google verification happens in `agentivity-rs`.

### 7.5 Delivery

One PR, one concern. Mark F8 `[x]` when merged and add its `CHANGELOG.md` entry that turn.

## 8. Known gaps (identified 2026-09-21; not scheduled, not approved)

Found while auditing the shipped stack. F8 covers OAuth `state`, PKCE, the `Secure` flag, the `postMessage` origin and the `email_verified` check; everything below is unscheduled. "Candidate" names a library worth evaluating, not a decision.

| Gap | Note | Candidate |
|---|---|---|
| `upsert` conflicts on email and overwrites `subject` | A second provider with the same email takes over the account; F8 only adds `email_verified` | — |
| `AuthUser` is not linked to `OrgMember`/`Vec<Grant>` | F7 works only if the application populates extensions | — |
| Callback page embeds `serde_json` output in an inline `<script>` | `</script>` in a provider-supplied name is not escaped | — |
| No request-CSRF layer on state-changing routes (`POST /auth/logout`, future POSTs) | Relies on `SameSite=Lax` alone | `tower-http` `csrf` feature (Fetch Metadata + Origin, stateless); API unverified |
| No security headers; no `Cache-Control: no-store` on `/auth/me` or the callback page | | `tower-http` `set-header` |
| No rate limiting on `/auth/*` | | `tower_governor` (axum compatibility unverified) |
| Error strings (SQL, provider bodies) returned to clients | `http.rs` callback error mapping | — |
| `connected_accounts.save` error is ignored; tokens stored in plaintext; no `user_id` link | Per-user disconnect cannot be authorized | — |
| Sessions: fixed expiry (`touch` unused), no purge of expired rows, `ip_address` always `""`, token is two UUIDv4s | | — |
| Identity comes from the userinfo endpoint; no `nonce` or `id_token` validation | | `openidconnect` 3.5 (matches `oauth2` 4.4) |
| `StandardUserInfo.id` is `Option<String>` | GitHub returns a numeric `id`; parsing is likely to fail | — |
| SDK methods without server routes: `signInWithPassword`, `connect*`, `disconnect*`, `revoke*`, `return_to` | | — |
| `examples/server.rs` uses `CorsLayer::very_permissive()` | Copy-paste hazard with credentialed cookies | `tower-http` `cors` (configured) |

## 9. Open questions

- **F8, pending approval:** the `Authorizer` trait surface in §7.3, and whether a `Matcher` trait is wanted now (proposed: no).
- **Request CSRF:** stateless (`tower-http` `csrf`) or pac4j-style token; and whether F8's callback-only `Authorizer` trait should stay, given that Tower layers already serve as authorizers elsewhere.
- **Gaps in §8:** which become issues, and which fold into F8 (#22).
- Resolved: `email_verified` `None` → reject; session cookie stays `SameSite=Lax`; axum upgraded to 0.8 with the duplicate `FromRef` removed.
