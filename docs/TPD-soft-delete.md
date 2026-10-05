# TPD — Soft delete

Status: `[~]` in progress (issue #70). Applies to every `Entity` and every table an application reads through megh.

## Rules
- Every entity has `created_at`, `updated_at` and `deleted_at` (nullable).
- **Deleting never removes a row.** It sets `deleted_at` (and `updated_at`). Whatever a physical delete would have removed goes with it, so the behaviour is the same as a hard delete, only reversible by an operator.
- `status` (draft, published, archived) belongs to the user's domain and is untouched; deletion is the application's concern and is never exposed as a status.
- A **separate batch job**, not part of this change, will later remove rows whose `deleted_at` is older than a retention period (the foreign keys' `ON DELETE CASCADE` then removes the dependants physically). megh may offer `purge(pool, older_than)` for it.

## Interface
```rust
pub struct Entity<ID, T> { pub id, pub data, pub created_at, pub updated_at, pub deleted_at: Option<DateTime<Utc>> }
pub const NOT_DELETED: &str = "deleted_at IS NULL";            // the condition reads add
trait TableEntity { async fn soft_delete(&self, pool) -> Result<Option<Self>, sqlx::Error>; ... }
```
- `soft_delete` finds the row by its primary key. It returns the deleted row, or `None` if the row does not exist or was already deleted (its earlier `deleted_at` stays).
- `update` and `upsert` do not touch a deleted row: an update of one fails with `RowNotFound`, an upsert does not revive it.
- Reads are SQL the application writes; each adds `NOT_DELETED` (qualified with the table name in a join).
- Migration `0008` adds `deleted_at` to every table megh maps with `Table` (`users`, `connected_accounts`, the billing tables), because `update` and `upsert` now check it. An application adds `deleted_at` (and `created_at`, `updated_at`) to its own entity tables. Only `deleted_at` is added here: a nullable column does not disturb the `INSERT ... SELECT *` that `insert` uses for rows without timestamps.

## How the cascade works
The foreign keys in the database are the declaration: no per-entity configuration. In one transaction, `soft_delete` takes one timestamp, marks the row, then follows every foreign key declared `ON DELETE CASCADE` that points at a table it just marked: the dependants whose key matches a row marked with that timestamp and are not yet deleted get the same `deleted_at` and `updated_at`. It repeats until no row changes, which also walks a table that references itself (a tree) to its leaves. Foreign keys without `ON DELETE CASCADE` are not followed (a physical delete would not follow them either). A dependant table without `deleted_at` or `updated_at` fails the delete with an error and nothing is changed: every entity table follows the rules above.

Libraries checked (cargo search, October 2026): `seaorm-soft-delete` and `diesel-softdelete` are for other ORMs, `flare-db` and `entity-derive` are whole-repository frameworks that would replace `Table` and `Entity`, `qraft` is a query builder; all are 0.1–0.2 and none cascades across relations. So this is built on megh's own `Table`/`TableEntity`.

## Fixed on the way
`update` and `upsert` wrote `SET (col) = (value)`, which Postgres rejects when an entity has a single data column; they now write `SET (cols) = ROW(values)`.

## Not in this change
Restore, the hard-delete batch job, timestamps (`created_at`, `updated_at`) on the megh tables that lack them (a delete cascading into such a table fails with an error until they have them), megh-go.

## Verification
`tests/soft_delete_test.rs` against Postgres (`#[sqlx::test]`): marking and keeping the row, a missing or already-deleted row, update and upsert on a deleted row, cascade through three levels with one timestamp, unrelated rows untouched, dependants already deleted keep their own timestamp, a foreign key without `ON DELETE CASCADE` is not followed, a self-referencing tree, a dependant table without `deleted_at` fails and changes nothing.
