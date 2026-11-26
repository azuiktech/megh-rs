# TPD — Persistence on PostgreSQL and SQLite (sqlx)

**Status:** built; every feature of megh runs on PostgreSQL and on SQLite, the database chosen at start-up from its URL (PRD feature 4).
**Modules:** `src/auth/db.rs`, `src/auth/entity.rs`, `src/auth/crud.rs`, `src/auth/filter.rs`, `src/auth/audit.rs`, `src/soft_delete.rs`, `megh-derive` (`#[entity]`, `sqlx_type!`), `migrations/postgres`, `migrations/sqlite`.

---

## Scope

Statements and generic CRUD for an entity written as one struct, on PostgreSQL and SQLite, on `sqlx`.

Non-negotiable:

1. The driver is chosen at runtime from the URL; nothing in megh assumes one.
2. Statements are built as sqlx builds them; only the names change (`sqlx::query_as` → `Db::query_as`). Standard SQL is written once; a driver whose SQL differs gets its own text (`.postgres(…)`, `.sqlite(…)`), and only there.
3. Every feature of each driver can be used (vendor texts); no lowest common denominator.
4. Values are bound and rows read through each type's own `sqlx::Type`; a type stored differently per driver says so with `sqlx_type!` or an annotation. Entity field types do not change.
5. The PostgreSQL database structure does not change (migrations, composite key `(tenant_id, id)`).
6. `soft_delete` and `timestamps` are optional; `create` generates the last key (UUIDv7); `created_at`, `updated_at`, `deleted_at` are set by the system.
7. Common CRUD, with AIP-158 pagination and AIP-160 filtering on list. Patch is `UPDATE <table> SET <the columns named> WHERE <key>`; `null` is the column's default.
8. Audit per table or in one shared table, in the write's transaction.

---

## Built

### Database (`src/auth/db.rs`)

```rust
let db = Db::connect(&env::var("DATABASE_URL")?).await?;   // postgres://… or sqlite:…
db.migrate().await?;                                          // migrations/postgres or migrations/sqlite
let user = Db::query_as::<User>("SELECT * FROM users WHERE email = $1::citext AND deleted_at IS NULL")
    .sqlite("SELECT * FROM users WHERE email = $1 AND deleted_at IS NULL")
    .bind(email)
    .fetch_optional(&db)
    .await?;
let mut tx = db.begin().await?;
Db::query("UPDATE … ").bind(id).execute(&mut tx).await?.rows_affected();
tx.commit().await?;
```

- `Db { Postgres(PgPool), Sqlite(SqlitePool) }` (each behind its feature, `postgres` (default) or `sqlite`; the drivers are megh's only features, at least one is needed), `Tx` the same for a transaction; `connect`, `migrate`, `begin`, `close`; `Tx::commit`, `Tx::rollback`. On SQLite a transaction starts `BEGIN IMMEDIATE`, so it holds the write lock where PostgreSQL locks rows (`FOR UPDATE`).
- `Db::query`, `Db::query_as::<T>`, `Db::query_scalar::<T>` as sqlx's: `.bind(v)` (owned or borrowed), `.postgres(sql)`, `.sqlite(sql)`, `.fetch_one/fetch_optional/fetch_all(on)`, `.execute(on)`. `on` is `&Db`, `&mut Tx` (anything `Into<On>`). Placeholders are `$n` (both drivers) or `?` (numbered for PostgreSQL). A SQLite text may hold several statements.
- A bound value is `Param<'q>`: `sqlx::Type` + `Encode` on every enabled driver. A read row is `Record`: `sqlx::FromRow` on every enabled driver.
- `sqlite_now!()`: the current time in SQLite SQL, written as sqlx writes a `DateTime<Utc>`.
- CRUD on `Db` and `Tx`: `get`, `list`, `select`, `create`, `upsert`, `replace`, `patch`, `delete` (`crud::*` with an actor for audit).

### Table and `#[entity]` (`src/auth/entity.rs`, `megh-derive/src/entity_attr.rs`)

`Table`: `TABLE_NAME`, `PRIMARY_KEY`, `COLUMNS` (names), `LISTS` (columns stored as `Array`), `SOFT_DELETE`, `AUDIT`, `params(self)` (each field bound through its type), `param(column, json)` (a patch or filter value through the field's type). `#[entity]` adds `#[derive(sqlx::FromRow)]` when missing and the timestamps.

- `#[sqlx(try_from = "S")]` on a field: read as `S`, written as `S::from(field)`. `Array<T>` is a PostgreSQL array and the text of a JSON array on SQLite: `#[sqlx(try_from = "Array<Uuid>")] pub service_ids: Vec<Uuid>`.
- `#[sqlx(json)]`: JSON on every driver; a patched value must be the field's type.
- `JsonText<T>`: a JSON document in a text column (megh-go's), read with `try_from = "Option<String>"`.
- Decimals (`Quantity`, `Percent`) are `numeric` on PostgreSQL and text on SQLite (`sqlx_type!`).

### CRUD, filter, audit, soft delete

- `crud` (`src/auth/crud.rs`) writes SQL text with placeholders; one statement per write with `RETURNING`; the key is `(tenant, id)` or the id alone; pagination forward by the last key (a uuid). Patch `null`: `DEFAULT` on PostgreSQL, `NULL` on SQLite, where `NOT NULL ON CONFLICT REPLACE` columns take their default.
- `filter` (`src/auth/filter.rs`): AIP-160 to a `Where` with each driver's text; a number compares numerically on SQLite (`CAST(… AS NUMERIC)`); `field:value` on a `LISTS` column (array / `json_each`) or a JSON array under `attributes`.
- `audit` (`src/auth/audit.rs`): one insert per change in the change's transaction.
- `soft_delete` (`src/soft_delete.rs`): the `ON DELETE CASCADE` keys from PostgreSQL's catalogs or SQLite's `pragma_foreign_key_list`, children marked with the parent's `deleted_at`.

### Migrations

`migrations/postgres`: unchanged. `migrations/sqlite`: the same tables, keys and columns; uuid is a 16-byte BLOB with a v7 default made in SQL, a time RFC 3339 text, jsonb / decimals / arrays text, citext `COLLATE NOCASE`; columns added by later PostgreSQL migrations are in their `CREATE TABLE` (every script runs again on each start). `reservations` keeps `starts_at`/`ends_at` where PostgreSQL keeps the `during` range.

### `sqlx_type!` (`megh-derive/src/sqlx_type.rs`, `src/lib.rs`)

A Rust type stored differently per database, written as plain `sqlx::Type` + `Encode` + `Decode`:

```rust
pub struct MyUuid(Uuid);

megh::sqlx_type!(MyUuid, match driver {
    Postgres => (
        |value: &MyUuid| -> Uuid { value.0 },
        |stored: Uuid| Ok(MyUuid(stored)),
    ),
    _ => (
        |value: &MyUuid| -> String { value.0.to_string() },
        |stored: String| Ok(MyUuid(stored.parse()?)),
    ),
});

let found: MyUuid = sqlx::query_scalar("SELECT referrer FROM clients WHERE referrer = ?").bind(MyUuid(id)).fetch_one(&pool).await?;
```

- `match driver`, one arm per driver (`Postgres`, `Sqlite`); `_` is whichever of the two is not named. A driver named twice, a second `_`, or a guard is a compile error. A driver whose megh feature is off gets no impls.
- Each arm is two closures with one typed parameter each: write `&T -> Stored`, read `Stored -> Result<T, _>` (`?` converts any error). The stored type is the write's return type and the read's parameter type; it is owned, not a reference. The closures become functions, so they capture nothing.
- `megh::__sqlx` re-exports sqlx; `__if_postgres!` / `__if_sqlite!` keep or drop a driver's impls by megh's features.
- Per arm the macro emits `Type<DB>` (the stored type's `type_info` and `compatible`), `Encode<DB>` (write, then the stored type's `Encode`), `Decode<DB>` (the stored type's `Decode`, then read), and on `Postgres` `PgHasArrayType` (the stored type's), so `Vec<T>` binds as an array. Nothing else.

### Tests

Every test that touches a database runs on SQLite (a database file of its own) and on PostgreSQL: `dual!(name)` in `tests/common`. Five run on PostgreSQL only (`postgres!(name)`): they read PostgreSQL's catalogs or replay megh-go's data and older PostgreSQL schemas. `tests/client_crud_dual_db_test.rs` covers CRUD, patch, soft and hard delete, validation, composite keys, common types, a database enum, a custom type stored encrypted (`sqlx_type!`), lists (`Array`), AIP-160 filters and `:`, pagination, audit and a caller's transaction; `tests/column_test.rs` the metadata `#[entity]` generates; `tests/sqlx_type_test.rs` the macro.

## Decided

- `create` generates the id; `upsert` writes the id given.
- Patch replaces whole columns; `null` is the column's default (PostgreSQL `DEFAULT`; SQLite `NOT NULL ON CONFLICT REPLACE`).
- Statements keep sqlx's shape; the PostgreSQL text of each statement is the one megh had; SQLite texts are added beside it.
- A list field keeps its `Vec<T>` type and is annotated `#[sqlx(try_from = "Array<T>")]`.

## Open

- `AddOn::price()` is renamed `amount()`: the generated column constant `AddOn::price` has the same name. To decide: keep the rename, or name the column constants differently.
- `crud` assumes a uuid last key and a tenant-first key for `list`; backward pagination is not planned.
- An `Option<T>` field cannot be stored through `try_from` (sqlx reads NULL before the conversion); none exists.
- On SQLite, decimal arithmetic in SQL (stock, totals) goes through `REAL`.
- `sea-query` is no longer used.
