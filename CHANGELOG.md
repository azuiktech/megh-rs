# Changelog

Append-only, newest first. One entry per PR, added the turn the PR is confirmed merged.

Format:

```
## YYYY-MM-DD
- Description of what shipped (PR #NNN, issue #NNN)
```

## 2026-09-21
- Connect, disconnect and revoke routes (`GET /auth/{p}/connect`, `POST /auth/{p}/disconnect`, `POST /auth/{p}/revoke`; session required, only the user's own account, matched by verified email) with `OAuthProviderConfig.revoke_url`; OAuth `state` + PKCE for login and connect (cookies set at start, checked at the callback: 403 on a wrong state, 400 without the verifier); the popup page escapes the JSON it embeds (PR #41, issue #40)
- Token refresh: `Accounts` with `auth`/`auth_for` returning `AccountAuth`, a `reqwest-middleware` layer that authenticates requests as a connected account and refreshes its token when expired (row lock so concurrent requests refresh once; a rejected refresh token marks the account disconnected and fails with `AccountError::InvalidGrant`); `ConnectedAccountRepo::save` keeps the stored refresh token on re-login (PR #39, issue #36)
- `reqwest` 0.12 → 0.13: `oauth2` is used without its bundled `reqwest` adapter, and the new `oauth_http_client(client)` lets a caller-configured `reqwest` 0.13 client perform token requests (PR #38, issue #37)
- Upgrade every dependency to its latest release: `oauth2` 4.4 → 5 (typestate client, `ProviderClient`; the shared HTTP client no longer follows redirects and has a 10 s timeout), `sqlx` 0.8 → 0.9 (derive-built SQL is asserted safe), `sha2` 0.10 → 0.11, `syn` 2 → 3, plus the rest of the lockfile; `cargo audit` now reports no vulnerabilities (was 5 and 1 warning, all through `oauth2 4.4` → `reqwest 0.11`). `reqwest` stays at 0.12 (required by `oauth2` 5.0.0). Add `AGENTS.md` with the always-latest rule; mark org O1 done (PR #35, issue #34)
- O1: align the org domain with megh-go: migration `0006` renames `org`/`org_members` to `organizations`/`organization_members` (or copies rows when megh-go's tables exist), aligns columns, stores `grants` as a JSON array in `text`, and creates `organization_invites`, `org_configs` and the billing tables (schema only); `OrgMember` is renamed `Member` (adds `role`, `invited_by`); `Org` gains megh-go's columns; new `Orgs::memberships` (PR #33, issue #28)
- Align `users` with megh-go: add `account_id`, `provider`, `password_hash` (text, nullable), unique `(account_id, provider)`, drop `subject` (migration `0005`, idempotent, backfills from `subject`); one user per email across providers, the first provider's identity is kept; `UserProfile`/`UpsertUserInput` use `account_id` + `provider`, `find_by_subject` becomes `find_by_account` (PR #32, issue #27)
- CSRF protection: re-export `tower-http`'s stateless `CsrfLayer` (`megh::auth::{CsrfLayer, ConfigError, ProtectionError}`) for any router, route or Tower service, and apply it by default to every `auth_router` route, configurable via `MeghAuthState::with_csrf` (PR #26, issue #25)
- Upgrade axum to 0.8 (route params now `/{x}`), reuse `axum::extract::FromRef` in place of the hand-written trait, ignore `.env.*`, replace the OAuth-hardening TPD with the feature-level `docs/TPD-authentication-authorization.md`, and add this changelog (PR #24, issue #23)
