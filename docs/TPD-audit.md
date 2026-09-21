# TPD — Audit

**Status:** A1 shipped. Ports megh-go's row-change audit triggers (Postgres only).
**Modules:** `src/audit.rs`, `migrations/0007_audit.sql`.

## 1. Delivery

| # | Feature | Status | PR / issue |
|---|---|---|---|
| A1 | Audit triggers with default names and overrides, `history`, transaction-scoped actor | `[x]` | #52 / #51 |

## 2. How it works

A trigger on a table writes every insert, update and delete to an audit table: the table name, the row's key as `record_id`, the action, the actor, the old and new row as JSON, and a timestamp. Migration 0007 creates megh-go's `audit_entries` table (same columns and indexes as its GORM AutoMigrate; checked against a real megh-go database) and three SQL functions:

- `megh_audit()`: the one trigger function every audited table shares (megh-go creates one function per table; the recorded data is the same). It writes with `clock_timestamp()`, so changes inside one transaction have distinct times.
- `megh_audit_install(table, audit_table, trigger_name, key)`: creates the audit table (`LIKE audit_entries INCLUDING ALL`) and the trigger, each only when missing. Every name goes through `format('%I')`, which closes the `Sprintf` injection risk in megh-go (review item G10).
- `megh_audit_history(audit_table, table, record)`: one row's changes, oldest first.

## 3. API (`megh::audit`, feature `postgres`)

```rust
// Every name has a default and can be overridden.
pub struct AuditTrigger { .. }
impl AuditTrigger {
    pub fn on(table: impl Into<String>) -> Self;          // audit table "audit_entries", trigger "trg_<table>_audit", key "id"
    pub fn dedicated(table: impl Into<String>) -> Self;   // audit table "<table>_audit"
    pub fn audit_table(self, name: impl Into<String>) -> Self;
    pub fn name(self, name: impl Into<String>) -> Self;
    pub fn key(self, column: impl Into<String>) -> Self;  // the column recorded as record_id
}
pub struct Audit { .. }                                    // holds the pool, like Orgs
impl Audit {
    pub fn new(pool: PgPool) -> Self;
    pub async fn install(&self, trigger: &AuditTrigger) -> Result<(), AuditError>;
    pub async fn history(&self, trigger: &AuditTrigger, record_id: &str) -> Result<Vec<AuditEntry>, AuditError>;
    pub async fn actor(connection: &mut PgConnection, actor: &str) -> Result<(), AuditError>;
}
pub struct AuditEntry { id, table_name, record_id, action: AuditAction, actor_id: Option<String>, old_data, new_data, created_at }
pub enum AuditAction { Insert, Update, Delete }            // stored as "INSERT", "UPDATE", "DELETE"
pub enum AuditError { Database(sqlx::Error) }              // #[non_exhaustive]
```

`old_data` and `new_data` are `JsonText<Value>`: null where there is no such row (no old row on insert, no new row on delete).

`install` leaves an existing trigger of the same name on that table alone, as megh-go does: a database that already has `trg_profiles_audit` keeps it and its function. Changing a trigger's configuration therefore needs the old trigger dropped first.

## 4. The actor

The trigger records `current_setting('megh.actor_id')`, and an empty or unset value means no actor. megh-go reads this setting but never sets it anywhere, so its `actor_id` is always NULL unless an application does it. `Audit::actor(&mut *tx, "user-id")` sets it with `set_config(.., is_local = true)`: call it on the transaction's connection before the writes. The setting ends with the transaction, so it cannot leak to the next request on a pooled connection; outside a transaction it has no lasting effect. After a transaction ends Postgres reports the setting as an empty string, not NULL, which is why the trigger treats an empty string as no actor.

## 5. Limits

- Postgres only. megh-go's SQLite triggers (which record only the primary key) are not ported.
- Only one key column is recorded (`key`), so composite keys are not supported.
- `history` reads one audit table for one row; there is no query by actor or time range yet.
