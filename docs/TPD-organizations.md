# TPD — Organizations, Membership & Invitations

**Status:** O1 (schema, `Member`, `Orgs::memberships`) is shipped; before it only the data types, the two tables and the grant check existed. Everything else in megh-go's organization domain is pending (§4) and will be ported (all of it, including login onboarding). The design in §6 is proposed and awaiting approval. megh-go is the reference (`/Users/abir/Documents/GitHub/megh-go`: `organizations.go`, `members.go`, `org.go`, `sessions.go`); the schema comparison below was made by running its real GORM `AutoMigrate` on Postgres.
**Modules:** `src/org`, `src/auth/authorizer.rs`, `migrations/0001_create_org.sql`.
**Related:** `TPD-authentication-authorization.md` (grants, authorizer, login). Billing (plans, subscriptions) is a separate feature area: `TPD-billing.md`.

## 1. Delivery

| # | Feature | Status | PR / issue |
|---|---|---|---|
| S1 | `Org` and `OrgMember` types; `org` and `org_members` tables | `[x]` | #2 / #1 |
| S2 | `OrgMember::has_grant`, used by the route authorizer through request extensions | `[x]` | #2 / #1, #20 / #19 |
| O1 | Schema alignment with megh-go (legacy tables renamed, or copied when megh-go's exist; columns; `grants` as JSON text; all megh-go tables incl. billing created schema-only), `OrgMember` renamed `Member`, `Orgs::memberships` | `[x]` | #33 / #28 |
| O2 | `Orgs`: org lifecycle, members, grants | `[ ]` | — |
| O3 | `Orgs`: invitations (redeem by token or email) and auto-join domains | `[ ]` | — |
| O4 | `Onboarding`: membership at login | `[ ]` | — |
| O5 | Memberships in `/auth/me` and the login response | `[~]` login response done (F14, #60/#30); `/auth/me` still returns `{user}` only | with #30 |

Features are not delivered in number order.

## 2. What exists in megh-rs

```rust
pub struct Org { id, name: Option<String>, description: Option<String>, timezone, sub_status, trial_ends_at, plan_id, created_by, created_at, updated_at: Option<..> }
pub struct Member { id, organization_id, user_id: Uuid, role: String, grants: Vec<String>, joined_at, invited_by: Option<Uuid> }
impl Member { pub fn has_grant(&self, required: &Grant) -> bool }
pub struct Orgs { .. }     // holds the injected pool
impl Orgs { pub fn new(pool: PgPool) -> Self; pub async fn memberships(&self, user_id: Uuid) -> Result<Vec<Member>, OrgError> }
```

`Org` and `Member` are read straight from megh-go's columns: `name`/`description` can be NULL, and so can `organizations.updated_at` (4 of 85 rows in a real megh-go database); a NULL, `[]` or `null` `grants` value means no grants. Everything else about orgs (create, members, grants, invitations, onboarding) is pending; there is no route and `/auth/me` returns no memberships. `Member` is also what the `authorizer` middleware reads from request extensions, which the application must supply. Tests: `has_grant` unit test, `authorizer_test.rs`, `course_authorizer_test.rs`, `org_test.rs`.

## 3. Schema parity with megh-go

Real megh-go DDL (Postgres, from GORM). "rs" is what megh-rs has today; O1 aligns it. megh-rs may keep extra columns and foreign keys on top.

| megh-go table | Columns (all nullable unless noted) | megh-rs before O1 |
|---|---|---|
| `organizations` | `id` uuid (pk), `name`, `description`, `timezone` default `'UTC'`, `sub_status` default `'trialing'`, `trial_ends_at`, `plan_id`, `created_by`, `created_at`, `updated_at` | `org`: `id`, `name`, `description`, `timezone`, `created_at` |
| `organization_members` | `id` (pk), `organization_id`, `user_id`, `role` default `'member'`, `grants` (**text**, JSON array of strings), `joined_at`, `invited_by`; unique `(organization_id, user_id)` | `org_members`: no `role`, no `invited_by`, `grants TEXT[]`; unique pair; FK to `org` with cascade |
| `organization_invites` | `id` (pk), `organization_id`, `email`, `token_hash` (unique), `status` default `'pending'`, `invited_by`, `expires_at`, `created_at`, `accepted_at` | absent |
| `org_configs` | `organization_id` (pk), `auto_join_domains` (text, JSON array) | absent |
| plans, plan_prices, add_ons, plan_add_ons, subscriptions, subscription_items | billing | absent; O1 creates them schema-only, the billing feature is out of scope |

O1 aligns everything above. megh-go created by GORM has no foreign keys on these tables (only between the billing tables); a database that came from megh-rs keeps its foreign key from `organization_members` to `organizations` (cascade). Deleting an org in a megh-go-created database leaves its members and invites orphaned.

## 4. Feature comparison with megh-go

Rows marked done match; the rest is pending. "megh-go" is the behaviour in code and tests, not only in its docs.

| Area | megh-go | megh-rs |
|---|---|---|
| Types | `OrgRecord` + `Org` domain object; `Member`, `Invite`, `Invitation`, `OrgConfig` | `Org`, `OrgMember` only (O1 renames to `Member`) |
| Grants on a member | `Member.Can(grant)` (Shiro matching) | done: `has_grant` (S2) |
| Create org | `CreateOrg(name, owner)`: creates the org and adds the owner as first member (two statements, not one transaction) | O2 |
| Seed org | `SeedOrg(name)`: org with no owner, for developer-bootstrapped apps | O2 |
| Get / remove org | `GetOrg(id)` (`ErrNotFound`), `RemoveOrg(id)` deletes only the org row | O2 |
| Update profile | `UpdateProfile(name, description, timezone *string)`: only the given fields | O2 |
| Members | `AddMember(user)` (empty grants), `Members()`, `Remove(memberID)` scoped to the org | O3 |
| Grants | `Grant(userID, g)` idempotent append, `Revoke(userID, g)`; `ErrNotFound` if not a member | O3 |
| Invite | `Invite(email, by)`: SHA-256-hashed 32-byte token, raw token returned once, 7-day expiry, status `pending` | O4 |
| Redeem | at login, a pending, unexpired invite matching the user's email is redeemed first, even if the user is already in another org (multi-org join); the status update is conditional on `pending`, so concurrent logins redeem once; the invite is `accepted` with `accepted_at` | O4, O5 |
| Onboarding order | `Sessions.Resolve`: pending invite, then existing membership (earliest by `joined_at`), then auto-join by email domain, then `DefaultOrgID`, then create a new org | O5 |
| Initial grants | `SessionsConfig.DefaultGrants` and `InitialGrants(email)` decide a new member's grants | O5 |
| Org config | `OrgConfig.AutoJoinDomains` read at login; no API writes it | O6 |
| Memberships | `Sessions.Memberships(userID)`; `/auth/me` returns `{user, memberships}` | O7 |
| Custom table names | `OrgModelsConfig` / `ModelDef` | not applicable to sqlx; not planned |

megh-go tests to mirror (`test/org_test.go`, `test/invite_test.go`): org lifecycle; members and grants; an expired invite is not redeemed; a valid invite is redeemed (status `accepted`, `accepted_at` set); concurrent logins produce one redemption and one member row; with an expired and a valid invite only the valid one is redeemed.

### megh-go behaviours not to copy as-is

- The invite token is created and hashed but never verified: redemption is by email match at login, so the token is only a hand-delivered secret that nothing checks. Decided: megh-rs redeems by token and by email (§6).
- Statuses `revoked` and `expired` are declared but nothing sets them; expiry is only enforced at query time, and there is no revoke-invite operation.
- `Role` exists (default `'member'`) but is never read or written.
- `createOrgWithAdmin` names an admin, but the creator gets only the configured default grants (none by default), so the first user of a new org cannot pass the `authorizer`. Proposed: the creator is an `owner` with configurable grants, default `["*"]` (§6).
- New orgs created at login have no name.
- Invite emails are matched exactly; no case normalisation.
- `CreateOrg` is not atomic (org row, then member row).
- Only the earliest membership is placed in the session; multi-org users see all memberships only through `/auth/me`.

## 5. Decisions

Decided: keep megh-go's table and column names and the same stored values where practical (`grants` is a JSON array of strings in a `text` column); the type is called `Member`; no repository per entity (§6); invites redeem by token and by email; login onboarding is ported in full; login may return memberships or an empty list; billing tables are created schema-only; documentation changes ride with the feature PRs, not their own PR.

Open:

1. Default grants for an org creator (§6): `["*"]` (proposed), or empty as in megh-go.
2. Whether redeeming by token requires the redeemer's email to match the invite's (proposed: no, the token is the credential).
3. Whether invite emails are compared case-insensitively (proposed: yes, unlike megh-go).

## 6. Design (proposed)

### 6.1 Shape

- **No repository per entity.** Data types (`Org`, `Member`, `Invite`, `Invitation`) are plain values with no behaviour and no database handle. Behaviour lives in two types that hold an injected `PgPool`:
  - `Orgs`, the organization aggregate: an org together with its members, grants, invites and auto-join domains is one unit of consistency, so one type owns all of it (megh-go's `App` + `Org`). Its methods are grouped by concern across files.
  - `Onboarding`, a use case that spans users and orgs: decide which org a user belongs to at login (megh-go's `Sessions.Resolve` membership part). It uses `Orgs` and holds the policy configuration.
- No trait is introduced yet; one implementation exists (a trait can be extracted at the consumer when a second one does).
- Errors: one `OrgError` (`NotFound`, `AlreadyMember`, `InviteNotRedeemable`, `Database(sqlx::Error)`) instead of raw `sqlx::Error`.
- Multi-step changes run in a transaction: create org + owner, redeem an invite, grant/revoke (row lock, so concurrent changes are not lost).

### 6.2 Types (`Invite`, `Invitation`, `InviteStatus` and `OrgChanges` arrive with the features that use them)

```rust
pub struct Org { id: Uuid, name: Option<String>, description: Option<String>, timezone: String,
                 sub_status: String, trial_ends_at: Option<DateTime<Utc>>, plan_id: Option<Uuid>,
                 created_by: Option<Uuid>, created_at: DateTime<Utc>, updated_at: Option<DateTime<Utc>> }
pub struct Member { id, organization_id, user_id: Uuid, role: String, grants: Vec<String>,   // JSON array in a text column
                    joined_at: DateTime<Utc>, invited_by: Option<Uuid> }
pub enum InviteStatus { Pending, Accepted, Revoked, Expired }                                // text column
pub struct Invite { id, organization_id: Uuid, email: String, status: InviteStatus, invited_by: Uuid,
                    expires_at, created_at: DateTime<Utc>, accepted_at: Option<DateTime<Utc>> }   // token_hash is never exposed
pub struct Invitation { invite: Invite (flattened), token: String }                          // raw token, returned once
pub struct OrgChanges { name: Option<String>, description: Option<String>, timezone: Option<String> }
```

`name` and `description` are `Option` because megh-go creates orgs at login with a NULL name; a NULL, `[]` or `null` grants value is read as empty.

### 6.3 API

```rust
impl Orgs {
    pub fn new(pool: PgPool) -> Self;
    pub fn with_owner_grants(self, grants: Vec<String>) -> Self;                 // default ["*"]

    // organizations (O2)
    pub async fn create(&self, name: &str, owner: Uuid) -> Result<Org, OrgError>;   // org + owner as first Member (role "owner"), one transaction
    pub async fn seed(&self, name: &str) -> Result<Org, OrgError>;                   // no owner
    pub async fn get(&self, id: Uuid) -> Result<Org, OrgError>;
    pub async fn update(&self, id: Uuid, changes: &OrgChanges) -> Result<Org, OrgError>;
    pub async fn remove(&self, id: Uuid) -> Result<(), OrgError>;

    // members and grants (O1: memberships; O2: the rest)
    pub async fn memberships(&self, user_id: Uuid) -> Result<Vec<Member>, OrgError>;   // ordered by joined_at
    pub async fn members(&self, org_id: Uuid) -> Result<Vec<Member>, OrgError>;
    pub async fn add_member(&self, org_id: Uuid, user_id: Uuid) -> Result<Member, OrgError>;
    pub async fn remove_member(&self, org_id: Uuid, member_id: Uuid) -> Result<(), OrgError>;
    pub async fn grant(&self, org_id: Uuid, user_id: Uuid, grant: &Grant) -> Result<Member, OrgError>;    // idempotent
    pub async fn revoke(&self, org_id: Uuid, user_id: Uuid, grant: &Grant) -> Result<Member, OrgError>;

    // invitations and auto-join (O3)
    pub async fn invite(&self, org_id: Uuid, email: &str, by: Uuid) -> Result<Invitation, OrgError>;
    pub async fn redeem_token(&self, token: &str, user_id: Uuid) -> Result<Member, OrgError>;
    pub async fn redeem_email(&self, user: &User) -> Result<Option<Member>, OrgError>;
    pub async fn revoke_invite(&self, invite_id: Uuid) -> Result<(), OrgError>;
    pub async fn set_auto_join_domains(&self, org_id: Uuid, domains: &[String]) -> Result<(), OrgError>;
}

impl Onboarding {                                                                  // O4
    pub fn new(orgs: Orgs) -> Self;
    pub fn with_default_org(self, org_id: Uuid) -> Self;
    pub fn with_default_grants(self, grants: Vec<String>) -> Self;               // for joiners; default empty
    pub fn with_initial_grants(self, f: impl Fn(&str) -> Option<Vec<String>> + Send + Sync + 'static) -> Self;   // by email
    pub async fn membership(&self, user: &User) -> Result<Member, OrgError>;
    // order (as megh-go): pending invite for the user's email, existing membership (earliest), auto-join by email domain, default org, create an org
}
```

### 6.4 Invitations

`invite` stores a SHA-256 hash of a 32-byte random token (64 hex characters), expiry 7 days, and returns the raw token once. Two ways to redeem, both a single conditional update on `status = 'pending' AND expires_at > now()` so an invite is redeemed once even under concurrent logins:

- **By token:** `redeem_token(token, user_id)` for an authenticated user (the token is the credential; the invite's email need not match).
- **By email:** `redeem_email(user)` at login, matching the invite's email case-insensitively. Because it grants org access from an email address, it must only run for a verified email (F8).

`revoke_invite` sets `revoked` (megh-go declares the status but never sets it); expiry stays a query-time condition.

### 6.5 Creator grants

An org creator is added as `role = 'owner'` with the configured owner grants, default `["*"]`; joiners get `default_grants` / `initial_grants` (empty by default). Reason, verified against megh-go's real rows: a user who logs in first gets an auto-created org and a member with `role = 'member'` and `grants = []`, and the `authorizer` denies every route unless a grant matches, so the creator cannot use their own org or grant anyone anything.

### 6.6 Schema migration (O1, `0006_align_org.sql`)

Idempotent, because databases are shared with megh-go and with applications:

- **Only megh-rs's `org` / `org_members` exist** (the usual megh-rs database): they are renamed to `organizations` / `organization_members`, not dropped and recreated, because application tables hold foreign keys to `org(id)` (a real `megh` database has `connections.org_id`, a real `kyrios` one has `connections` and `courses`); those keys follow a rename. Columns are widened to megh-go's (`name`/`description` nullable `text`, `timezone` `text`; new `sub_status`, `trial_ends_at`, `plan_id`, `created_by`, `updated_at`, `role`, `invited_by`), and `grants TEXT[]` becomes `text` with `to_json(grants)::text` (compact array, same as megh-go).
- **megh-go's tables already exist next to the old ones** (a real `kyrios` database): rows are copied into them (`ON CONFLICT DO NOTHING`, so repeatable) and the old tables are left in place, because their dependents keep pointing at them. Those foreign keys must be re-pointed by the owner of those tables.
- **Neither exists:** `CREATE TABLE IF NOT EXISTS`.
- `organization_invites`, `org_configs` and the billing tables (`plans`, `plan_prices`, `add_ons`, `plan_add_ons`, `subscriptions`, `subscription_items`) are created from megh-go's real DDL, schema only.

Checked on scratch copies of three real databases: 85 organizations and 97 of 97 members decode through `Org` and `Orgs::memberships` after migrating.

### 6.7 Delivery and tests

O1 to O5 are separate PRs, each updating this document and `CHANGELOG.md`. Tests are written first, against a real Postgres (`#[sqlx::test]`), mirroring megh-go's: org lifecycle; members and grants; expired invite not redeemed; valid invite redeemed (`accepted`, `accepted_at`); concurrent redemption yields one member row; expired plus valid invite redeems only the valid one; onboarding order.
