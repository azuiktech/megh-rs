# TPD — Tenants, Membership & Invitations

**Renamed (T1, #209):** the org is now the **tenant**, the partition every row belongs to (`Tenant`, `Tenants`, `TenantError`, `tenant_id`, tables `tenants`, `tenant_members`, `tenant_invites`, `tenant_configs`, routes `/tenants/{tenant_id}/...`). **Org** is kept only for the future physical body that owns one or more tenants and hierarchies: decided, not built, designed later and separately. megh-go's names (`organizations`, `OrgRecord`, `Org`) appear below only where they describe megh-go.
**Status:** `[~]` partly shipped. Shipped: the schema (O1), the rename and compacted migrations (T1), contact and address on `Tenant` (O6), `Tenants::create` and `Tenants::memberships`, lifecycle hooks (#238), a personal tenant at a user's first OAuth login, and memberships in the `/auth/me` and basic-login responses (O5). Not built: members and grants management, invitations, auto-join domains, tenant update and removal, and the rest of onboarding (invite redeem and auto-join at login); the designs in §6 for those are proposed and awaiting approval. megh-go is the reference (`/Users/abir/Documents/GitHub/megh-go`: `organizations.go`, `members.go`, `org.go`, `sessions.go`); the schema comparison below was made by running its real GORM `AutoMigrate` on Postgres.
**Modules:** `src/tenant` (`model.rs`, `member.rs`, `tenants.rs`, `lifecycle.rs`), `src/auth/authorizer.rs`, `src/auth/http.rs`, `migrations/0003_tenant.sql`.
**Related:** `TPD-authentication-authorization.md` (grants, authorizer, login). Billing (plans, subscriptions) is a separate feature area: `TPD-billing.md`.

## 1. Delivery

| # | Feature | Status | PR / issue |
|---|---|---|---|
| S1 | `Tenant` and `Member` types; `tenants` and `tenant_members` tables (first built as `Org`/`OrgMember`, `org`/`org_members`) | ✅ | #2 / #1 |
| S2 | `Member::has_grant`, used by the route authorizer through request extensions | ✅ | #2 / #1, #20 / #19 |
| O1 | Schema alignment with megh-go (columns; `grants` as JSON text; invites and configs tables; all megh-go tables incl. billing created schema-only), `Member`, `Tenants::memberships` | ✅ | #33 / #28 |
| O2 | `Tenants`: lifecycle, members, grants. `create` (tenant plus owner member, one transaction) is built; `seed`, `get`, `update`, `remove`, `members`, `add_member`, `remove_member`, `grant`, `revoke` are not | `[~]` missing everything but `create` | #225 |
| O3 | `Tenants`: invitations (redeem by token or email) and auto-join domains; only the `tenant_invites` and `tenant_configs` tables exist | `[ ]` | — |
| O4 | `Onboarding`: membership at login. Built: a user with no membership gets a tenant named after their email at the first OAuth login (`give_a_tenant`, `src/auth/http.rs`). Not built: invite redeem, auto-join by domain, default tenant, initial grants, the `Onboarding` type | `[~]` | #225 |
| O5 | Memberships in `/auth/me` and in the basic-login response | ✅ | #60 / #30, #225 |
| O6 | Contact and address on `Tenant`: `contact: Contact` (flattened `email`, `home_phone`, `work_phone`, `mobile_phone`) and `address: Option<Address>` (JSONB), the layout and types of booking's `Client` (`booking::value`); inline in `0003_tenant.sql`, all nullable and not in megh-go (its `OrgRecord` ignores them). Read-only until `Tenants::update` (O2); `:open` still keeps `bill_to` as drafted | ✅ | #208 / #207 |
| T1 | Org renamed tenant in types, tables, columns, routes and the token's audience; migrations compacted to one file per module (`0001_auth` to `0008_connection_spec`), with no history kept: an existing database is renamed by hand | ✅ | #210 / #209 |
| L1 | Lifecycle hooks: one `Change` per change, run after the commit (`src/tenant/lifecycle.rs`) | ✅ | #238 |

Features are not delivered in number order.

## 2. What exists in megh-rs

```rust
pub struct Tenant { id, name: Option<String>, description: Option<String>, contact: Contact, address: Option<Address>, timezone, sub_status, trial_ends_at, plan_id, created_by, created_at, updated_at: Option<..> }
pub struct Member { id, tenant_id, user_id: Uuid, role: String, grants: Vec<String>, joined_at, invited_by: Option<Uuid> }
impl Member { pub fn has_grant(&self, required: &Grant) -> bool }
pub struct Tenants { .. }     // holds the injected pool and the optional lifecycle
impl Tenants {
    pub fn new(pool: PgPool) -> Self;
    pub fn with_lifecycle(self, lifecycle: Option<Lifecycle>) -> Self;
    pub async fn memberships(&self, user_id: Uuid) -> Result<Vec<Member>, TenantError>;      // earliest first; none for a deleted user
    pub async fn create(&self, name: &str, owner: Uuid) -> Result<Tenant, TenantError>;
}
pub enum TenantError { Database(sqlx::Error), Lifecycle(LifecycleError) }                  // #[non_exhaustive]
```

`Tenant` and `Member` keep megh-go's columns: `name`/`description` can be NULL, and so can `updated_at` (4 of 85 rows in a real megh-go database); a NULL, `[]` or `null` `grants` value means no grants. `Tenants::create(name, owner)` makes a tenant with its owner as first member (role `owner`, grants `["*"]`) in one transaction, then runs the lifecycle hook. An OAuth login by a user with no membership creates one named after the user's email (`src/auth/http.rs`), and `/auth/me` and the basic-login response return the user's memberships. Everything else about tenants (update, removal, members, grants, invitations, the rest of onboarding) is not built; there is no tenant route. `Member` is also what the `authorizer` middleware reads from request extensions, which the application must supply. Tests: `has_grant` unit test, `authorizer_test.rs`, `course_authorizer_test.rs`, `tenant_test.rs`, `lifecycle_test.rs`.

## 3. Schema parity with megh-go

Real megh-go DDL (Postgres, from GORM). "megh-rs" is the table `0003_tenant.sql` creates. megh-rs keeps extra columns and foreign keys on top.

| megh-go table | Columns (all nullable unless noted) | megh-rs |
|---|---|---|
| `organizations` | `id` uuid (pk), `name`, `description`, `timezone` default `'UTC'`, `sub_status` default `'trialing'`, `trial_ends_at`, `plan_id`, `created_by`, `created_at`, `updated_at` | `tenants`: the same, plus `email`, `home_phone`, `work_phone`, `mobile_phone`, `address` (JSONB); no `deleted_at` |
| `organization_members` | `id` (pk), `organization_id`, `user_id`, `role` default `'member'`, `grants` (**text**, JSON array of strings), `joined_at`, `invited_by`; unique `(organization_id, user_id)` | `tenant_members`: the same with `tenant_id` (FK to `tenants`, cascade); unique `(tenant_id, user_id)` |
| `organization_invites` | `id` (pk), `organization_id`, `email`, `token_hash` (unique), `status` default `'pending'`, `invited_by`, `expires_at`, `created_at`, `accepted_at` | `tenant_invites`: the same with `tenant_id`; schema only |
| `org_configs` | `organization_id` (pk), `auto_join_domains` (text, JSON array) | `tenant_configs`: `tenant_id` (pk), `auto_join_domains`; schema only |
| plans, plan_prices, add_ons, plan_add_ons, subscriptions, subscription_items | billing | created by `0005_billing.sql`; see `TPD-billing.md` |

megh-go created by GORM has no foreign keys on these tables (only between the billing tables); megh-rs has one from `tenant_members` to `tenants` (cascade). Deleting an org in a megh-go-created database leaves its members and invites orphaned.

## 4. Feature comparison with megh-go

✅ rows match; the rest is not built. "megh-go" is the behaviour in code and tests, not only in its docs.

| Area | megh-go | megh-rs |
|---|---|---|
| Types | `OrgRecord` + `Org` domain object; `Member`, `Invite`, `Invitation`, `OrgConfig` | `Tenant`, `Member` ✅; `Invite`, `Invitation`, `OrgConfig` equivalents not built (tables only) |
| Grants on a member | `Member.Can(grant)` (Shiro matching) | ✅ `has_grant` (S2) |
| Create org | `CreateOrg(name, owner)`: creates the org and adds the owner as first member (two statements, not one transaction) | ✅ `Tenants::create`, one transaction, owner grants `["*"]` |
| Seed org | `SeedOrg(name)`: org with no owner, for developer-bootstrapped apps | O2 |
| Get / remove org | `GetOrg(id)` (`ErrNotFound`), `RemoveOrg(id)` deletes only the org row | O2 |
| Update profile | `UpdateProfile(name, description, timezone *string)`: only the given fields | O2 |
| Members | `AddMember(user)` (empty grants), `Members()`, `Remove(memberID)` scoped to the org | O2 |
| Grants | `Grant(userID, g)` idempotent append, `Revoke(userID, g)`; `ErrNotFound` if not a member | O2 |
| Invite | `Invite(email, by)`: SHA-256-hashed 32-byte token, raw token returned once, 7-day expiry, status `pending` | O3 |
| Redeem | at login, a pending, unexpired invite matching the user's email is redeemed first, even if the user is already in another org (multi-org join); the status update is conditional on `pending`, so concurrent logins redeem once; the invite is `accepted` with `accepted_at` | O3, O4 |
| Onboarding order | `Sessions.Resolve`: pending invite, then existing membership (earliest by `joined_at`), then auto-join by email domain, then `DefaultOrgID`, then create a new org | `[~]` O4: existing membership, else a new tenant named after the email; the other steps not built |
| Initial grants | `SessionsConfig.DefaultGrants` and `InitialGrants(email)` decide a new member's grants | O4 |
| Org config | `OrgConfig.AutoJoinDomains` read at login; no API writes it | O3 |
| Memberships | `Sessions.Memberships(userID)`; `/auth/me` returns `{user, memberships}` | ✅ O5: `/auth/me` and the basic-login response |
| Custom table names | `OrgModelsConfig` / `ModelDef` | not applicable to sqlx; not planned |

megh-go tests to mirror (`test/org_test.go`, `test/invite_test.go`): org lifecycle; members and grants; an expired invite is not redeemed; a valid invite is redeemed (status `accepted`, `accepted_at` set); concurrent logins produce one redemption and one member row; with an expired and a valid invite only the valid one is redeemed.

### megh-go behaviours not to copy as-is

- The invite token is created and hashed but never verified: redemption is by email match at login, so the token is only a hand-delivered secret that nothing checks. Decided: megh-rs will redeem by token and by email (§6, not built).
- Statuses `revoked` and `expired` are declared but nothing sets them; expiry is only enforced at query time, and there is no revoke-invite operation.
- `Role` exists (default `'member'`) but is never read or written.
- `createOrgWithAdmin` names an admin, but the creator gets only the configured default grants (none by default), so the first user of a new org cannot pass the `authorizer`. megh-rs: the creator is an `owner` with grants `["*"]` (built in `create`; configurable owner grants are proposed, §6).
- New orgs created at login have no name (megh-rs names the tenant after the user's email).
- Invite emails are matched exactly; no case normalisation.
- `CreateOrg` is not atomic (org row, then member row); megh-rs `create` is.
- Only the earliest membership is placed in the session; multi-org users see all memberships only through `/auth/me`.

## 5. Decisions

Decided (✅ for what is built): keep megh-go's table and column names and the same stored values where practical (`grants` is a JSON array of strings in a `text` column); the type is called `Member`; no repository per entity (§6); invites redeem by token and by email; login onboarding is ported in full; login may return memberships or an empty list; billing tables are created schema-only; documentation changes ride with the feature PRs, not their own PR.

Open:

1. Whether the owner grants of a new tenant are configurable (§6, `with_owner_grants`, proposed); `create` always uses `["*"]` today.
2. Whether redeeming by token requires the redeemer's email to match the invite's (proposed: no, the token is the credential).
3. Whether invite emails are compared case-insensitively (proposed: yes, unlike megh-go).

## 6. Design (proposed, not built)

Only `Tenants::new`, `with_lifecycle`, `create` and `memberships` and the `Tenant`/`Member` types exist (§2). Everything else in this section is proposed, not built.

### 6.1 Shape

- **No repository per entity.** Data types (`Tenant`, `Member`, `Invite`, `Invitation`) are plain values with no behaviour and no database handle. Behaviour lives in two types that hold an injected `PgPool`:
  - `Tenants`, the tenant aggregate: a tenant together with its members, grants, invites and auto-join domains is one unit of consistency, so one type owns all of it (megh-go's `App` + `Org`). Its methods are grouped by concern across files.
  - `Onboarding`, a use case that spans users and tenants: decide which tenant a user belongs to at login (megh-go's `Sessions.Resolve` membership part). It uses `Tenants` and holds the policy configuration.
- No trait is introduced yet; one implementation exists (a trait can be extracted at the consumer when a second one does).
- Errors: `TenantError` has only `Database(sqlx::Error)` and `Lifecycle(LifecycleError)` today; `NotFound`, `AlreadyMember` and `InviteNotRedeemable` are proposed with the features that need them.
- Multi-step changes run in a transaction: create tenant + owner (built), redeem an invite, grant/revoke (row lock, so concurrent changes are not lost).

### 6.2 Types (`Tenant` and `Member` are built; `Invite`, `Invitation`, `InviteStatus` and `TenantChanges` are proposed and arrive with the features that use them)

```rust
pub struct Tenant { id: Uuid, name: Option<String>, description: Option<String>, contact: Contact, address: Option<Address>, timezone: String,
                 sub_status: String, trial_ends_at: Option<DateTime<Utc>>, plan_id: Option<Uuid>,
                 created_by: Option<Uuid>, created_at: DateTime<Utc>, updated_at: Option<DateTime<Utc>> }
pub struct Member { id, tenant_id, user_id: Uuid, role: String, grants: Vec<String>,   // JSON array in a text column
                    joined_at: DateTime<Utc>, invited_by: Option<Uuid> }
pub enum InviteStatus { Pending, Accepted, Revoked, Expired }                                // text column
pub struct Invite { id, tenant_id: Uuid, email: String, status: InviteStatus, invited_by: Uuid,
                    expires_at, created_at: DateTime<Utc>, accepted_at: Option<DateTime<Utc>> }   // token_hash is never exposed
pub struct Invitation { invite: Invite (flattened), token: String }                          // raw token, returned once
pub struct TenantChanges { name: Option<String>, description: Option<String>, timezone: Option<String> }
```

`name` and `description` are `Option` because megh-go creates orgs at login with a NULL name; a NULL, `[]` or `null` grants value is read as empty.

### 6.3 API

```rust
impl Tenants {
    pub fn new(pool: PgPool) -> Self;
    pub fn with_owner_grants(self, grants: Vec<String>) -> Self;                 // proposed, not built; default ["*"]

    // tenants (O2)
    pub async fn create(&self, name: &str, owner: Uuid) -> Result<Tenant, TenantError>;   // BUILT: tenant + owner as first Member (role "owner"), one transaction
    pub async fn seed(&self, name: &str) -> Result<Tenant, TenantError>;                   // no owner
    pub async fn get(&self, id: Uuid) -> Result<Tenant, TenantError>;
    pub async fn update(&self, id: Uuid, changes: &TenantChanges) -> Result<Tenant, TenantError>;
    pub async fn remove(&self, id: Uuid) -> Result<(), TenantError>;

    // members and grants (memberships is built; the rest is O2, proposed)
    pub async fn memberships(&self, user_id: Uuid) -> Result<Vec<Member>, TenantError>;   // BUILT: ordered by joined_at
    pub async fn members(&self, tenant_id: Uuid) -> Result<Vec<Member>, TenantError>;
    pub async fn add_member(&self, tenant_id: Uuid, user_id: Uuid) -> Result<Member, TenantError>;
    pub async fn remove_member(&self, tenant_id: Uuid, member_id: Uuid) -> Result<(), TenantError>;
    pub async fn grant(&self, tenant_id: Uuid, user_id: Uuid, grant: &Grant) -> Result<Member, TenantError>;    // idempotent
    pub async fn revoke(&self, tenant_id: Uuid, user_id: Uuid, grant: &Grant) -> Result<Member, TenantError>;

    // invitations and auto-join (O3, proposed)
    pub async fn invite(&self, tenant_id: Uuid, email: &str, by: Uuid) -> Result<Invitation, TenantError>;
    pub async fn redeem_token(&self, token: &str, user_id: Uuid) -> Result<Member, TenantError>;
    pub async fn redeem_email(&self, user: &User) -> Result<Option<Member>, TenantError>;
    pub async fn revoke_invite(&self, invite_id: Uuid) -> Result<(), TenantError>;
    pub async fn set_auto_join_domains(&self, tenant_id: Uuid, domains: &[String]) -> Result<(), TenantError>;
}

impl Onboarding {                                                                  // O4, proposed, not built
    pub fn new(tenants: Tenants) -> Self;
    pub fn with_default_tenant(self, tenant_id: Uuid) -> Self;
    pub fn with_default_grants(self, grants: Vec<String>) -> Self;               // for joiners; default empty
    pub fn with_initial_grants(self, f: impl Fn(&str) -> Option<Vec<String>> + Send + Sync + 'static) -> Self;   // by email
    pub async fn membership(&self, user: &User) -> Result<Member, TenantError>;
    // order (as megh-go): pending invite for the user's email, existing membership (earliest), auto-join by email domain, default tenant, create a tenant
}
```

### 6.4 Invitations (proposed, not built)

`invite` stores a SHA-256 hash of a 32-byte random token (64 hex characters), expiry 7 days, and returns the raw token once. Two ways to redeem, both a single conditional update on `status = 'pending' AND expires_at > now()` so an invite is redeemed once even under concurrent logins:

- **By token:** `redeem_token(token, user_id)` for an authenticated user (the token is the credential; the invite's email need not match).
- **By email:** `redeem_email(user)` at login, matching the invite's email case-insensitively. Because it grants tenant access from an email address, it must only run for a verified email (F8).

`revoke_invite` sets `revoked` (megh-go declares the status but never sets it); expiry stays a query-time condition.

### 6.5 Creator grants

Built: a tenant creator is added as `role = 'owner'` with grants `["*"]`. Proposed: configurable owner grants (`with_owner_grants`); joiners get `default_grants` / `initial_grants` (empty by default). Reason, verified against megh-go's real rows: a user who logs in first gets an auto-created org (megh-go) and a member with `role = 'member'` and `grants = []`, and the `authorizer` denies every route unless a grant matches, so the creator cannot use their own org or grant anyone anything.

### 6.6 Schema migration

Built: `migrations/0003_tenant.sql` (compacted in T1; there are only `0001` to `0008`). It is idempotent (`CREATE TABLE IF NOT EXISTS`) because databases are shared with megh-go and with applications. It creates `tenants` (with contact and address inline), `tenant_members`, `tenant_invites` and `tenant_configs` from megh-go's real DDL plus megh-rs's extras (§3), and sets the v7 id default on tables made earlier. The billing tables are in `0005_billing.sql`. No data is renamed or copied: an existing database is renamed by hand (T1).

At O1 (before the rename) 85 organizations and 97 of 97 members from three real databases decoded through the types and `memberships`.

### 6.7 Delivery and tests

O2 to O5 are separate PRs, each updating this document and `CHANGELOG.md`. Tests are written first, against a real Postgres (`#[sqlx::test]`), mirroring megh-go's: tenant lifecycle; members and grants; expired invite not redeemed; valid invite redeemed (`accepted`, `accepted_at`); concurrent redemption yields one member row; expired plus valid invite redeems only the valid one; onboarding order.

## Decided (from the clarifications)

- **Lifecycle hooks (#238, ✅).** One `Change` per change: tenant created, updated, removed; member added, updated, removed; subscribed; subscription changed, cancelled. The app's `Lifecycle` is a plain `fn(PgPool, Change)` set on `MeghAuthState.lifecycle` and runs after the change is committed. A failing hook's error is returned by the call; the change stays committed. Code: `src/tenant/lifecycle.rs`.
- **What emits today.** `Change::TenantUpdated`, `TenantRemoved`, `MemberUpdated` and `MemberRemoved` are declared but never emitted: no code calls them. There is no API to add or remove members or to change grants, and `tenant_invites` and `tenant_configs` are schema only.
- **Open:** where the tenant hierarchy lives. A physical Org that owns tenants is designed later (decided, not built).
