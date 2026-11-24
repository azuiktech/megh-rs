# TPD — Multi-database persistence (sea-query + sqlx)

**Status:** built; every entity of megh is on it (issue #261). Supersedes the SeaORM draft and the `Entity<ID, T>` / `TableEntity` / `#[derive(Table)]` / trigger-audit code.
**Modules:** `megh-derive` (`#[entity]`, `#[derive(Column)]`), `src/auth/column.rs`, `src/auth/db.rs`, `src/auth/crud.rs`, `src/auth/filter.rs`, `src/auth/audit.rs`, `src/auth/entity.rs`, `src/soft_delete.rs`.

---

## Scope

Generic create, read, list, replace, patch and delete for an entity written as one struct, on PostgreSQL and SQLite, on `sea-query` and `sqlx` (no SeaORM).

Non-negotiable:

1. The database structure does not change (migrations, composite key `(tenant_id, id)`).
2. The type structure does not change (`pub contact: Contact` stays flattened; `Name`, `Gender`, `Address`, `Attributes` stay).
3. The macro works with the sqlx annotations (`json`, `try_from`, `flatten`, `rename`, `skip`, `rename_all`, `type_name`) and generates the metadata, including the primary key.
4. `soft_delete` is optional: the macro adds `deleted_at`; reads, updates and deletes use `deleted_at IS NULL`; delete sets `deleted_at` and `updated_at`. Without it, delete is `DELETE FROM <table> WHERE <key>`.
5. `timestamps` is optional: the macro adds `created_at` and `updated_at`. The macro never adds `id`: key fields are declared and named in `keys`.
6. Common CRUD, with AIP-158 pagination and AIP-160 filtering on list.
7. Patch is `UPDATE <table> SET <the columns the request names> WHERE <key>`: no read first, each column replaced whole, nothing inside a column value merged.
8. CRUD works on the entity directly over axum, with validation (`validator`). One struct per entity.
9. The code that runs a statement, reads a row and builds a query is written once; databases differ only in a few converters (below). No `FromRow<PgRow>`-style bound on entities.
10. Audit per table or in one shared table, in the write's transaction.
11. Fields convert to and from `sea_query::Value` directly; the struct is never serialized to JSON to be written or read.
12. A new type needs one `impl Column` (or `text_column!`, `newtype_column!`, `#[derive(Column)]`); no other logic changes, and the macro matches no type names.
13. The system decides: `create` generates the last key (UUIDv7); `created_at`, `updated_at`, `deleted_at` are set by the system when writing. What an entity, a request body or a patch carries in them is discarded.
14. Enums follow the sqlx annotations: `#[sqlx(rename_all = "...")]`, `#[sqlx(rename = "...")]`, `#[sqlx(type_name = "...")]`.

---

## Built

### Type mapping (`src/auth/column.rs`)

```rust
pub enum Kind { Text, Uuid, Date, Time, Timestamp, Bool, Int, Float, Decimal, Bytes, Json, Array(Elem) }   // Elem: Text, Uuid, Int

pub trait Column: Sized {
    const KIND: Kind;
    const TYPE_NAME: Option<&'static str> = None;      // a database enum type, written cast to it
    fn to_value(&self) -> sea_query::Value;
    fn from_value(value: sea_query::Value) -> Result<Self, ColumnError>;
}
```

- Implemented for `String`, `bool`, `i32`, `i64`, `f64`, `Decimal`, `Uuid`, `NaiveDate`, `NaiveTime`, `DateTime<Utc>`, `Vec<u8>`, `Option<T>`, and `Vec<T>` of text, uuids or integers (a native array on PostgreSQL, JSON text on SQLite); `JsonText<T>` (a JSON document in a text column).
- Newtypes: `text_column!(Name, Phone, Email, Currency)` (`TryFrom<String>` + `AsRef<str>`), `newtype_column!(Micros: i64, Percent: Decimal, Capacity: i32)` (`TryFrom` + `Into` of the inner type).
- Enums: `#[derive(crate::Column)]` stored as text, named by the sqlx annotations on the enum (`Gender`, `PaymentStatus`, …).
- `#[sqlx(json)]` fields convert that one column value with serde. Nothing else is serialized.
- `Table::from_json(column, json)` (generated) converts a patch or filter value by the field's own type, so a custom type validates or encrypts a patched value the same way as a written one.
- A new database primitive is a `Kind` variant and one arm in the row reader; a new domain type is one `impl`.

### Table and `#[entity]` (`src/auth/entity.rs`, `megh-derive/src/entity_attr.rs`)

`Table`: `TABLE_NAME`, `PRIMARY_KEY`, `COLUMNS: &[ColumnMeta { name, kind, type_name, is_primary_key }]`, `SOFT_DELETE`, `AUDIT`, `values()`, `from_values()`, `from_json()`, all generated.

`#[entity(table = "clients", keys = ["tenant_id", "id"], timestamps, soft_delete, audit = shared)]` above the derives:

- injects `created_at: DateTime<Utc>` (serde default: now), `updated_at` and `deleted_at: Option<DateTime<Utc>>` (serde default: none), with `#[schema(read_only)]` when the struct derives `ToSchema`;
- removes the `#[sqlx(...)]` attributes unless the struct derives `sqlx::FromRow` itself (then they stay; those written `cfg_attr(…, sqlx(…))` are read too);
- emits `IntoResponse` (JSON) for a struct that derives `Serialize` and has keys, when the crate's `axum` feature is on;
- `table` defaults to the snake-cased name, `keys` to `["id"]`, `keys = []` for a fragment flattened into other entities (`Contact`); `audit` is `shared`, `per_table`, or a table name in quotes.

### Databases (`src/auth/db.rs`)

`Backend` is implemented for `sqlx::Postgres` and `sqlx::Sqlite` by one macro (`backend!`): the query builder, how to run a statement and read a row (written once, by `Kind`), and the converters that differ:

| | PostgreSQL | SQLite |
|---|---|---|
| time of a write | `CURRENT_TIMESTAMP` (what the soft-delete cascade matches) | the clock |
| patch `null` | `DEFAULT` (the column's default) | `NULL` |
| a list | native array | JSON text |
| a decimal | `numeric` | text |
| `field:value` on a list | `value = ANY(list)` | `EXISTS (… json_each …)` |
| soft-delete cascade | `ON DELETE CASCADE` keys from the catalogs (`src/soft_delete.rs`) | none |

Everything in `crud` and `filter` is generic over `DB: Backend`; a pool, a transaction or a connection is accepted (`impl Acquire<'c, Database = DB>`). `AppContext` keeps its `PgPool`; a SQLite pool is passed the same way.

### CRUD (`src/auth/crud.rs`)

```rust
pub async fn get<'c, T, DB>(db: impl Acquire<'c, Database = DB>, key: &Key) -> ApiResult<T>;
pub async fn list<'c, T, DB>(db: …, request: &ListRequest) -> ApiResult<Page<T>>;
pub async fn select<'c, T, DB>(db: …, build: impl FnOnce(&mut SelectStatement)) -> ApiResult<Vec<T>>;
pub async fn create<'c, T, DB>(db: …, data: T, actor: Option<&str>) -> ApiResult<T>;
pub async fn upsert<'c, T, DB>(db: …, data: T) -> ApiResult<T>;       // a row whose id the caller decides; a deleted row is not revived
pub async fn replace<'c, T, DB>(db: …, data: T, actor: Option<&str>) -> ApiResult<T>;
pub async fn patch<'c, T, DB>(db: …, body: Changes, actor: Option<&str>) -> ApiResult<T>;
pub async fn delete<'c, T, DB>(db: …, key: &Key, actor: Option<&str>) -> ApiResult<T>;
```

- Each write is one statement with `RETURNING`; a row is read first only when the entity is audited.
- The key is `(tenant, id)`, or the id alone for a table keyed by it. Pagination orders by the last key column, which must be a uuid; the cursor is forward-only.
- Patch: unknown field → 400; `created_at`, `updated_at`, `deleted_at` are ignored; each value is converted by the field's type; `null` is `DB::reset()`; the returned row is validated and the transaction rolled back (400) if it is not valid.
- A check or not-null violation is 400; other database errors are `ApiError::Database`.
- `crud::Row` is `Table + Validate + Serialize + Send + Sync`, for code generic over entities (`booking::price::Priced`).

### Filter (`src/auth/filter.rs`)

AIP-160 to `sea_query::Condition`. A name in `COLUMNS` compares with the value converted by the column's type (a database enum is cast to its type). Any other name compares `attributes -> '<key>'` with the JSON value, the key a bound parameter. `field:value` is membership of a list column or of a JSON array in `attributes`; on another column it is a 400.

### Audit (`src/auth/audit.rs`)

`AuditPolicy { Disabled, Shared, PerTable, Dedicated(&'static str) }` per entity. `crud` writes the row (`id` v7, `table_name`, `record_id`, `action`, `actor_id`, `old_data`, `new_data`, `created_at`; migration 0002's shape) in the change's transaction; a failed audit write fails the change. The migration's trigger functions stay in the database, unused.

### Ported

All tenant entities (`Client`, `Resource`, `Provider`, `Service`, `Schedule`, `Blockout`, `Appointment`, `Product`, `Stock`, `Price`, `Tax`, `Invoice`, `InvoiceLine`, `Payment`), `User`, the billing plans (`Plan`, `PlanPrice`, `AddOn`, `PlanAddOn`, `Subscription`, `SubscriptionItem`) and `ConnectedAccount` are `#[entity]` structs: no `XData`, no `Entity<ID, T>`, no `Keyed<T>`. Raw PostgreSQL SQL that needs locks (`FOR UPDATE`, reservations, stock) stays as SQL and reads the same structs through `sqlx::FromRow`.

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

`tests/column_test.rs` (mapping and macro metadata); `tests/client_crud_dual_db_test.rs` (every scenario on SQLite and PostgreSQL: create, get, replace, patch, soft and hard delete, validation, composite keys, generated id and system timestamps, uuid / decimal / float / int / bool / date / time / timestamp / list columns, a database enum named by sqlx annotations, a custom `Secret` stored encrypted, AIP-160 filter and `:`, pagination, `select`, audit policies and rollback, a caller's transaction); `tests/soft_delete_test.rs` (cascade, PostgreSQL); `tests/bytea_test.rs`; `tests/sqlx_type_test.rs` (`_` arm on SQLite: type, write, read, read error; a named `Sqlite` arm; the `Postgres` arm; `Vec<T>` as a PostgreSQL array).

---

## Decided

- `create` generates the id; `upsert` writes the id given.
- Patch replaces whole columns; `null` resets to the database default where the database can say so (PostgreSQL), else `NULL`.
- Struct literals of an entity list `created_at`, `updated_at`, `deleted_at` (deserializing fills them in); `updated_at` and `deleted_at` are `Option`, `created_at` is not.
- `Tx`-style per-database code is replaced by the `Backend` macro; adding a database is one `backend!` call and its converters.
- SQLite stores uuids as BLOB (the form sqlx decodes), timestamps, dates, times and decimals as TEXT.

## Open

- `crud` assumes a uuid last key and a tenant-first key for `list`.
- A `Vec<T>` column holds text, uuids or integers only; a list of another type needs a new `Elem`.
- Reads open a transaction (`begin`) because `Acquire::acquire` makes handler futures fail the `Send` check when called with `&mut Transaction`; a lighter read path needs sqlx to fix that.
- `sea-query` 1.0 needs `sea-query-binder` 0.8 (release candidate only): stay on 0.32 / 0.7 (AGENTS.md).
- Backward pagination is not planned.
- The first-draft interface names (`Db`, `Tx`, `Kind::value_of`, `AuditPolicy` in `entity.rs`) are gone.
