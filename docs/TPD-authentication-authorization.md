# TPD — Authentication & Authorization

**Status:** F1–F11, F13, F15, F16 and F17 shipped (F12 is superseded by F16). F14 (basic login route with `Member`, aligned with megh-go) is planned. Features are not delivered in number order.
**Modules:** `src/auth`, `src/account`, `src/org`, `ui/sdk/src/auth.ts`, `migrations/0001–0004`.
**Depends on:** `Entity<ID, T>` (`src/entity.rs`) for `User`.

This is the living design for everything that answers "who is calling" (authentication) and "may they do this" (authorization). Change-level detail belongs in `CHANGELOG.md`; this document changes when the feature's shape changes.

## 1. Delivery

| # | Feature | Status | PR / issue |
|---|---|---|---|
| F1 | Org tenancy and Shiro-style permission grants | `[x]` | #2 / #1 |
| F2 | User identity (`users`, `UserRepo`) | `[x]` | #4 / #3 |
| F3 | OAuth 2.0 client and connected accounts | `[x]` | #6 / #5 |
| F4 | Sessions; `User` as `Entity` (sessions replaced by F16) | `[x]` | #8 / #7 |
| F5 | Axum auth router, session cookie, `AuthUser` extractor | `[x]` | #10 / #9 |
| F6 | OAuth popup `postMessage` protocol, `ui/sdk`, configurable redirect URI | `[x]` | #11 |
| F7 | Route-based grant authorizer middleware | `[x]` | #20 / #19 |
| F8 | OAuth callback hardening: `state`, PKCE, verified email, `postMessage` origin, safe result page, sanitized errors | `[x]` | #53 / #22 |
| F9 | CSRF protection (`tower-http` `csrf` layer; replaced by F16) | `[x]` | #26 / #25 |
| F10 | Users table aligned with megh-go (`account_id`, `provider`, `password_hash`; `subject` dropped); one user per email across providers | `[x]` | #32 / #27 |
| F11 | Org and member tables aligned with megh-go; `OrgMember` renamed `Member`; all remaining megh-go tables created (schema only); membership lookup (see `TPD-organizations.md`, O1) | `[x]` | #33 / #28 |
| F12 | Sessions table aligned with megh-go (`id text`, `data text`, opaque token); superseded by F16, whose store table cannot match | `[!]` | #31 |
| F13 | Password storage: `set_password` / `verify_password` (bcrypt) | `[x]` | #44 / #29 |
| F14 | Basic login route (`basic_login_router`) returning user and memberships | `[ ]` | #30 |
| F15 | Token refresh: `Accounts` and a `reqwest-middleware` layer (`AccountAuth`); a re-login keeps the stored refresh token | `[x]` | #39 / #36 |
| F16 | Sessions on `tower-sessions` (Postgres store) and token CSRF (`axum-tower-sessions-csrf`) in `auth_router` | `[x]` | #43 / #42 |
| F17 | JWT access token as a cache in front of the session (`jwt_session`, `jsonwebtoken`), renewed from the session | `[x]` | #50 / #49 |

## 2. Design vocabulary (pac4j)

Terms follow pac4j (https://www.pac4j.org/docs/).

| pac4j | Contract | In megh |
|---|---|---|
| Authenticator / client | turns a request into a verified user | OAuth login and callback (F3, F5) |
| Matcher | does security apply to this request? | Axum routing and `route_layer`. No `Matcher` trait. |
| Authorizer | given a request (and profile), allow or deny; deny is 403 | `authorizer` middleware over `Grant`s (F7). |

pac4j's `csrfCheck` is an app-level double-submit check on POSTs. Its OAuth clients keep `state` and PKCE inside the client.

## 3. Architecture

- **Stack:** axum 0.8, `tower-sessions` 0.14 with `axum-tower-sessions-csrf` =0.1.1, sqlx 0.8 (Postgres; held back, see `AGENTS.md`), `oauth2` 5 (no bundled HTTP client; `oauth_http_client` adapts the caller's `reqwest` 0.13 client), `reqwest` 0.13 (rustls), `sha2`/`hex` for token hashing, `reqwest` for userinfo.
- **Feature flags:** `postgres` gates repos and `sqlx::FromRow`; `client` gates `reqwest` and `fetch_user_info`; `axum` gates the router, extractor and authorizer middleware. The router (`auth::http`) needs both `axum` and `postgres`. All three are default.
- **Authentication path:** browser → `/auth/{provider}/login` → provider → `/auth/{provider}/callback` → code exchange → userinfo → `users` upsert → `connected_accounts` upsert → `session.cycle_id()` and `user_id` stored in the `tower-sessions` session (cookie set by the app's `SessionManagerLayer`). Later requests: session `user_id` → `UserRepo::get_by_id` → `AuthUser`.
- **Authorization path:** the application puts a `Member` (or a `Vec<Grant>`) into request extensions; the `authorizer` middleware derives the required `Grant` from the matched route and method and checks it. megh does not populate those extensions; nothing links `AuthUser` to `OrgMember` yet (see §8).

## 4. Data model

| Table | Columns | Constraints |
|---|---|---|
| `organizations` | `id`, `name`, `description`, `timezone`, `sub_status`, `trial_ends_at`, `plan_id`, `created_by`, `created_at`, `updated_at` | PK `id` (was `org`, see `TPD-organizations.md`) |
| `organization_members` | `id`, `organization_id`, `user_id`, `role`, `grants` (text, JSON array), `joined_at`, `invited_by` | unique `(organization_id, user_id)`; keeps a foreign key to `organizations` (cascade) where the table was renamed from `org_members`; `user_id` has no FK (was `org_members`) |
| `users` | `id`, `account_id`, `provider`, `email`, `password_hash`, `display_name`, `photo_url`, `created_at`, `updated_at` | unique `(account_id, provider)`, unique `email` (F10; before F10: `subject` instead of `account_id`/`provider`, unique) |
| `connected_accounts` | `account_id`, `provider`, `email`, `access_token`, `refresh_token`, `token_type`, `expiry`, `created_at`, `updated_at`, `disconnected_at` | PK `(account_id, provider)`; no `user_id` |
| `sessions` | `id`, `user_id`, `token_hash`, `expires_at`, `user_agent`, `ip_address`, `metadata JSONB`, `created_at`, `updated_at` | FK `user_id` → `users` (cascade); unique `token_hash`. Created by migration 0004, unused since F16 |
| `tower_sessions.session` | `id text`, `data bytea`, `expiry_date timestamptz` | created by the app calling `PostgresStore::migrate()` (F16) |

`megh::migrate(&pool)` runs every migration; `examples/server.rs` uses `sqlx::migrate!`. Migrations are idempotent because databases are shared with megh-go in practice (`kyrios` holds both stacks' tables).

### Schema parity with megh-go

megh-go has no SQL migrations; its schema is what GORM `AutoMigrate` creates from the model tags. The comparison below was made by running that migration on Postgres and diffing against ours. Goal: same tables and columns wherever possible; values need not match (portable values are a bonus). megh-rs may add columns and foreign keys on top.

| Table | megh-go | megh-rs |
|---|---|---|
| `users` | `id`, `account_id`, `provider`, `email`, `password_hash`, `created_at` (all `text` but ids/timestamps) | F10 |
| `connected_accounts` | `(account_id, provider)` key, token columns | aligned |
| `organizations` | `org` in megh-rs, fewer columns | F11 |
| `organization_members` | `org_members` in megh-rs; `role`, `invited_by`; `grants` is `text` holding a JSON array | F11 |
| `organization_invites`, `org_configs`, plans, prices, add-ons, subscriptions | present | F11 (schema only) |
| `sessions` | `id text`, `data text`, timestamps | not aligned: F16 uses the `tower-sessions` store table |

## 5. Implemented features and their APIs

### F1 — Org tenancy and grants (`org`, `auth::grant`)

```rust
pub struct Org { id: Uuid, name: Option<String>, description: Option<String>, timezone: String, sub_status: String,
                 trial_ends_at: Option<DateTime<Utc>>, plan_id: Option<Uuid>, created_by: Option<Uuid>, created_at: DateTime<Utc>, updated_at: Option<DateTime<Utc>> }
pub struct Member { id, organization_id, user_id: Uuid, role: String, grants: Vec<String>, joined_at: DateTime<Utc>, invited_by: Option<Uuid> }
impl Member { pub fn has_grant(&self, required: &Grant) -> bool }

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
pub struct UserProfile { account_id: Option<String>, provider: Option<String>, email: String, display_name: String, photo_url: String }
pub struct UpsertUserInput { provider: String, account_id: String, email: String, display_name: Option<String>, photo_url: Option<String> }

impl UserRepo<'_> {                            // feature "postgres"
    pub fn new(pool: &PgPool) -> UserRepo;
    pub async fn get_by_id(&self, id: Uuid) -> Result<Option<User>, sqlx::Error>;
    pub async fn find_by_email(&self, email: &str) -> Result<Option<User>, sqlx::Error>;
    pub async fn find_by_account(&self, provider: &str, account_id: &str) -> Result<Option<User>, sqlx::Error>;
    pub async fn upsert(&self, input: &UpsertUserInput) -> Result<User, sqlx::Error>;   // conflict key: email
    pub async fn update_profile(&self, id: Uuid, display_name: &str, photo_url: &str) -> Result<User, sqlx::Error>;
}
```

`upsert` keeps one user per email across providers: the first provider's `(provider, account_id)` is kept and never overwritten by later logins (it only fills them in when unset), and existing `display_name`/`photo_url` are kept when the new value is empty. Each provider's tokens live in their own `connected_accounts` row, keyed by `(account_id, provider)` and linked by email. Linking by email is only safe for verified emails (F8).

### F13 — Password storage (`auth::user`, feature `postgres`)

```rust
impl UserRepo<'_> {
    pub async fn set_password(&self, user_id: Uuid, password: &str) -> Result<(), PasswordError>;      // bcrypt hash into users.password_hash
    pub async fn verify_password(&self, email: &str, password: &str) -> Result<User, PasswordError>;
}
pub enum PasswordError { UserNotFound, NoPassword, WrongPassword, Hash(bcrypt::BcryptError), Database(sqlx::Error) }   // #[non_exhaustive]
```

`bcrypt` 0.19; hashes verify in both directions with megh-go's `bcrypt.GenerateFromPassword` (tested against a hash it produced). `verify_password` reports why it failed and leaves the policy to the caller. megh-go stores an unset password as `''`, which counts as `NoPassword`. When the user or password is missing it still runs one bcrypt verification against a dummy hash, so response time does not tell which emails exist. Hashing runs on the calling task (about 0.25 s at cost 12); a caller on an async runtime can wrap the call in `spawn_blocking`. No lockout or rate limiting (F14 and hardening).

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
    pub fn build_client(&self, redirect_url: Option<RedirectUrl>) -> Result<ProviderClient, OAuthError>;   // ProviderClient = oauth2 5 BasicClient with auth and token endpoints set
}

pub struct AuthUrlOptions<'a> {
    scopes: &'a [&'a str], pkce: Option<&'a PkceCodeChallenge>,
    offline_access: bool,     // access_type=offline
    incremental: bool,        // include_granted_scopes=true
    prompt: Option<&'a str>,
}
pub fn build_authorization_url(client: &ProviderClient, csrf_token: CsrfToken, opts: AuthUrlOptions) -> Url;

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

### F15 — Token refresh (`account::accounts`, features `postgres` + `client`)

The caller builds and configures their own `reqwest` client and adds megh's layer; every request is authenticated as the connected account, with the access token refreshed first when it has expired. Callers never handle tokens.

```rust
pub struct Accounts { .. }        // holds the injected pool and the configured providers
impl Accounts {
    pub fn new(pool: PgPool, providers: Arc<HashMap<String, OAuthProviderConfig>>) -> Self;
    pub fn with_http_client(self, http: reqwest::Client) -> Self;         // token requests to the provider; must not follow redirects
    pub fn with_expiry_margin(self, margin: Duration) -> Self;            // default 60 s
    pub fn auth(&self, provider: &str, account_id: &str) -> AccountAuth;
    pub async fn auth_for(&self, user: &User, provider: &str) -> Result<AccountAuth, AccountError>;   // account found by the user's email
}
pub struct AccountAuth { .. }     // impl reqwest_middleware::Middleware
pub enum AccountError { NotFound, Disconnected, NoRefreshToken, NotConfigured(String), InvalidGrant(String), Provider(String), Database(sqlx::Error) }

let api = reqwest_middleware::ClientBuilder::new(my_reqwest_client).with(accounts.auth("google", &account_id)).build();
api.get(url).send().await?;
```

Refresh runs under a row lock (`SELECT … FOR UPDATE`), so concurrent requests refresh once, across processes; the provider call happens while the lock is held (bounded by the client's timeout). A refresh token the provider sends is stored, otherwise the stored one is kept. `invalid_grant` marks the account `disconnected_at` and the request fails with `AccountError::InvalidGrant`, carried in `reqwest_middleware::Error::Middleware` (`downcast_ref::<AccountError>()`); later requests fail with `Disconnected` without calling the provider. `ConnectedAccountRepo::save` no longer overwrites a stored refresh token with an empty one (a re-login: providers such as Google only send it on first consent). Not included: retry on 401, a connect flow that forces re-consent, token encryption at rest (A7).

### F4 — Sessions

Replaced by F16: the `session` module (`SessionRepo`, `Session`, `SessionView`, token helpers) no longer exists.

### F5 — Axum auth router (`auth::http`, features `axum` + `postgres`)

```rust
pub struct MeghAuthState {                              // Clone; all fields pub
    pool: PgPool, redirect_after_login: String, app_origin: String,
    providers: Arc<HashMap<String, OAuthProviderConfig>>, http_client: reqwest::Client,
}
impl MeghAuthState {
    pub fn new(pool: PgPool) -> Self;                   // redirect "/", origin "http://localhost:8080"
    pub fn with_redirect_after_login(self, impl Into<String>) -> Self;
    pub fn with_app_origin(self, impl Into<String>) -> Self;
    pub fn add_provider(self, OAuthProviderConfig) -> Self;
}
pub fn auth_router(state: MeghAuthState) -> Router;    // needs a tower-sessions SessionManagerLayer around it
pub struct AuthUser { pub user: User }                  // FromRequestParts<S> where MeghAuthState: FromRef<S>
pub struct AuthMeResponse { pub user: User }
pub use axum::extract::FromRef;                         // replaces the former hand-written trait
```

| Route | Behaviour |
|---|---|
| `GET /auth/{provider}`, `GET /auth/{provider}/login` | 303 to the provider authorization URL (`access_type=offline`, `include_granted_scopes=true`, `prompt=select_account`, provider default scopes). 404 unknown provider; 500 bad callback URL. |
| `GET /auth/{provider}/callback`, `GET /auth/{provider}/token` | Same handler. Query: `code`, `state`, `error`, `error_description`. Exchanges the code, fetches userinfo, upserts the user, saves the `connected_accounts` row, starts the session (`cycle_id`, `user_id`). 200 HTML. 400 provider `error` or missing `code`; 404 unknown provider; 502 exchange or userinfo failure; 500 database or session failure. Callback URL defaults to `{app_origin}/auth/{provider}/token` unless `redirect_url` is set. |
| `GET /auth/me` | 200 `AuthMeResponse`. 401 no `user_id` in the session or missing user; 500 database or session error. |
| `POST /auth/logout` | Flushes the session, 200 `{"status":"ok"}`. Needs the CSRF token (F16). |
| `GET /auth/csrf-token` | 200 with the session's CSRF token as the body (F16). |

The session cookie, its name, lifetime and `Secure`/`SameSite` flags are the `SessionManagerLayer`'s settings, not megh's.

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

`authorizer` is an Axum `from_fn` middleware. It reads a `Member` (else a `Vec<Grant>`) from request extensions and requires a `MatchedPath`. The requested grant is `resource:action[:instance]`: `resource` is the last static path segment, or the segment before the last path parameter; `instance` is the last path parameter's value.

Responses: 401 `not authenticated` (no `Member` or grants in extensions), 403 `not permitted`, 404 `route not found` (no matched path). Verified by `tests/authorizer_test.rs` and `tests/course_authorizer_test.rs` with a fake injector.

### F9, F16 — Sessions and CSRF (`auth::http`; #26 / #25, then #43 / #42)

F9 first used the stateless `tower-http` `CsrfLayer` (`Sec-Fetch-Site` / `Origin`). F16 replaced it with the synchronizer token pattern from `axum-tower-sessions-csrf` and moved sessions to `tower-sessions`; the app mounts the router under the session layer:

```rust
let store = PostgresStore::new(pool.clone());            // tower-sessions-sqlx-store; store.migrate() once at startup
let app = megh::auth_router(state).layer(SessionManagerLayer::new(store));
```

`auth_router` puts `CsrfMiddleware::middleware` on every route it serves (inside the session layer). A client calls `GET /auth/csrf-token` once per session and sends the value as `x-csrf-token` on every POST, PUT, PATCH and DELETE; a missing or wrong token gets 403. The token lives in the session (constant-time comparison, no cookie of its own), so it dies with `logout`. `GET`, `HEAD` and `OPTIONS` are not checked, so the OAuth login redirect and callback are outside the check (they are covered by F8).

Versions are held back (`AGENTS.md`): the only Postgres store for `tower-sessions` is `tower-sessions-sqlx-store` 0.15, which needs `tower-sessions-core` 0.14 and `sqlx` 0.8; `axum-tower-sessions-csrf` 0.1.3+ needs `tower-sessions` 0.15. The store's `tower_sessions.session` table (`id`, `data`, `expiry_date`) cannot match megh-go's `sessions` table. `events_router` (`POST /sub`) is not covered (§9).

### F17 — JWT access token (`auth::token`, features `axum` + `postgres`; #50 / #49)

A short-lived signed token that answers requests without a database lookup, and is renewed from the session when it expires. It is not a session: nothing is stored, and the transport (a cookie here) is independent of the token.

```rust
pub struct AccessToken { sub: Uuid, iss: String, aud: Vec<String>, exp: u64, iat: u64, client_id: String, scope: String }   // RFC 9068
impl AccessToken { pub fn new(member: &Member, ttl: Duration) -> Self; pub fn grants(&self) -> Vec<Grant> }
pub struct JwtSession { .. }                                   // Clone; signing key, lifetime, pool
impl JwtSession { pub fn new(secret: &[u8], ttl: Duration, pool: PgPool) -> Self }
pub async fn jwt_session(State<JwtSession>, CookieJar, Session, Request, Next) -> Result<(CookieJar, Response), StatusCode>
pub const JWT_COOKIE: &str = "jwt_token";

let api = Router::new().route(..).layer(from_fn(megh::authorizer)).layer(from_fn_with_state(jwt, megh::auth::jwt_session));
let app = api.layer(SessionManagerLayer::new(store));           // jwt_session sits inside the session layer
```

The claims are megh-go's (`sub` member id, `iss` "megh", `aud` organization id, `scope` the grants space-separated, HS256), so a token minted by either stack verifies in the other with the same secret. Per request: a valid token puts its grants in the request extensions, where `authorizer` already reads them (zero database queries); a missing or expired token is renewed from the session's `user_id` and the user's first membership, sets a fresh cookie and continues; a token with a bad signature, issuer or format is 401 even when a session exists; no session, or no membership, is 401. Expiry has no leeway. `POST /auth/logout` clears the cookie (when the request carried one).

Cookie: `HttpOnly`, `Secure`, `SameSite=Lax`, `Path=/`, `Max-Age` = the lifetime (megh-go omits `Secure`).

Revocation is the token lifetime, as in megh-go: there is no `jti` or denylist, ending the session stops renewal but a token already issued works until it expires, so keep the lifetime short. Not included: minting at login (the first request renews), a bearer-header transport, key rotation.

`jsonwebtoken` 11 with its `aws_lc_rs` backend (already in the build through `rustls`); its `rust_crypto` backend would pull in the `rsa` crate.

## 6. Test coverage (shipped)

`tests/grant_test.rs`, `authorizer_test.rs`, `course_authorizer_test.rs` (F1, F7); `account_oauth_test.rs` (F3); `auth_router_test.rs` (F5); `csrf_test.rs` (F9, F16); `jwt_session_test.rs` (F17); `user_test.rs` (F10); unit tests in `auth/grant.rs`, `auth/user.rs`, `org/member.rs`. Router tests use a lazy pool and never query. Tests that need Postgres use `#[sqlx::test]` (`user_test.rs`): each test gets its own throwaway database on the server named by `DATABASE_URL` (read from the environment or `.env`), so `cargo test` needs a reachable Postgres. `test_connected_account_repo_persistence` also uses `DATABASE_URL`, defaulting to the `kyrios` dev database, and skips only when the database is unreachable. Router tests mount the router under an in-memory `tower-sessions` store. No shipped test covers the OAuth callback against a live database or the Postgres session store.

## 7. F8 — OAuth callback hardening (`auth::http`; #53 / #22)

The checks come from `oauth2` (state and PKCE types, constant-time comparison) and `tower-sessions` (where the values wait); megh wires them.

| Threat | Control |
|---|---|
| Login CSRF: an attacker completes their own OAuth flow in the victim's browser | `oauth_login` generates `CsrfToken::new_random()`, sends it as `state` and keeps it in the session; the callback compares the returned `state` with `CsrfToken ==`, which is constant time under `oauth2`'s `timing-resistant-secret-traits` feature |
| Authorization-code interception | PKCE: `PkceCodeChallenge::new_random_sha256()` (S256) at login, the verifier kept in the session and sent with `set_pkce_verifier` at the code exchange |
| Guessing or replaying `state` | The stored attempt is per provider and is consumed by the first callback, right or wrong: a wrong guess ends that login. No stored attempt is 400, a mismatch is 403, and the provider is not called in either case |
| Session fixation | `cycle_id()` at login (F16) |
| Linking an account through an unverified email | The userinfo `email_verified` must be `true`; `false` or absent is 403 and no user is created |
| The result page leaking the profile to any window | `postMessage` targets `web_origin` (`MeghAuthState::with_web_origin`, default `app_origin`), never `*` |
| Script injection through a provider-supplied name | The result page carries its data in HTML attributes escaped by `html-escape`; its script is static |
| Provider, SQL or session errors reaching the client | Fixed messages (`sign-in failed`, `could not save the user`, ...); the cause is logged with `tracing`. A failed `connected_accounts` save is now an error, not ignored |

`web_origin` is the origin of the page that opened the popup: the backend cannot read it, and `app_origin` is the backend's own, so a web app on another origin must set it.

The session cookie must be `SameSite=Lax`: `tower-sessions` defaults to `Strict`, and a Strict cookie is not sent on the cross-site redirect back from the provider, so the login attempt would never be found. `examples/server.rs` sets it (`SessionManagerLayer::new(store).with_same_site(SameSite::Lax)`).

Responses: 400 provider `error`, missing `code`, or no login in progress; 403 `state` mismatch or unverified email; 404 unknown provider; 502 exchange or userinfo failure; 500 database or session failure.

Not covered: `nonce` and `id_token` validation (identity still comes from the userinfo endpoint; `openidconnect` would add it), and encryption of stored tokens.

## 8. Known gaps (identified 2026-09-21; not scheduled, not approved)

Found while auditing the shipped stack. F8 has closed the OAuth `state`, PKCE, `email_verified`, `postMessage` and error-handling gaps; everything below is unscheduled. "Candidate" names a library worth evaluating, not a decision.

| Gap | Note | Candidate |
|---|---|---|
| `AuthUser` is not linked to `Member`/`Vec<Grant>` | F7 works only if the application populates extensions | — |
| `events_router` `POST /sub` has no CSRF check | F16 covers `auth_router` only; `events_router` has no session layer, so protecting it changes its signature | F16's `CsrfMiddleware` |
| No security headers; no `Cache-Control: no-store` on `/auth/me` or the callback page | | `tower-http` `set-header` |
| No rate limiting on `/auth/*` | | `tower_governor` (axum compatibility unverified) |
| Tokens stored in plaintext; `connected_accounts` has no `user_id` link | Per-user disconnect cannot be authorized | — |
| Sessions: fixed expiry (`touch` unused), no purge of expired rows, `ip_address` always `""`, token is two UUIDv4s | | — |
| Identity comes from the userinfo endpoint; no `nonce` or `id_token` validation | | `openidconnect` 3.5 (matches `oauth2` 4.4) |
| `StandardUserInfo.id` is `Option<String>` | GitHub returns a numeric `id`; parsing is likely to fail | — |
| SDK methods without server routes: `signInWithPassword`, `connect*`, `disconnect*`, `revoke*`, `return_to` | | — |
| `examples/server.rs` uses `CorsLayer::very_permissive()` | Copy-paste hazard with credentialed cookies | `tower-http` `cors` (configured) |

## 9. Open questions

- **`events_router` CSRF:** the SDK posts `/sub` cross-origin with credentials and would have to fetch and send the token; protecting it also needs a session layer around `events_router`.
- **Gaps in §8:** which become issues.
- Resolved: `email_verified` `None` → reject; session cookie stays `SameSite=Lax`; axum upgraded to 0.8 with the duplicate `FromRef` removed; request CSRF uses the stateless `tower-http` layer, on by default in `auth_router`.
