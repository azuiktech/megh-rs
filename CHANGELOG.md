# Changelog

Append-only, newest first. One entry per PR, added the turn the PR is confirmed merged.

Format:

```
## YYYY-MM-DD
- Description of what shipped (PR #NNN, issue #NNN)
```

## 2026-09-21
- CSRF protection: re-export `tower-http`'s stateless `CsrfLayer` (`megh::auth::{CsrfLayer, ConfigError, ProtectionError}`) for any router, route or Tower service, and apply it by default to every `auth_router` route, configurable via `MeghAuthState::with_csrf` (PR #26, issue #25)
- Upgrade axum to 0.8 (route params now `/{x}`), reuse `axum::extract::FromRef` in place of the hand-written trait, ignore `.env.*`, replace the OAuth-hardening TPD with the feature-level `docs/TPD-authentication-authorization.md`, and add this changelog (PR #24, issue #23)
