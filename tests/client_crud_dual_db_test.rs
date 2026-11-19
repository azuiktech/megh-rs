//! docs/TPD-persistence.md: generic CRUD, patch, soft delete, pagination, AIP-160 filter and audit,
//! each scenario run on SQLite and on PostgreSQL through the same generic code.

use std::time::Duration;

use aip_160::parse_filter;
use chrono::{DateTime, Duration as Span, NaiveDate, NaiveTime, TimeZone, Utc};
use futures_util::future::join_all;
use megh::auth::rest::{ApiError, Changes, Key, ListRequest, PageRequest};
use megh::booking::client::Client;
use megh::booking::value::Name;
use megh::auth::column::{Column, ColumnError, Kind};
use megh::auth::crud;
use megh::auth::db::Backend;
use megh::auth::entity::ColumnMeta;
use megh::sea_query::{Alias, Asterisk, Condition, Expr, Func, Order, Query, SelectStatement, Value};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use serde_json::{Value as Json, json};
use sqlx::sqlite::SqlitePoolOptions;
use sqlx::{PgPool, Pool, SqlitePool};
use uuid::Uuid;
use validator::Validate;

/// A type the library does not know: one `impl Column` is all it takes to store it.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
struct Cents(i64);

impl Column for Cents {
    const KIND: Kind = Kind::Int;
    fn to_value(&self) -> Value {
        self.0.to_value()
    }
    fn from_value(value: Value) -> Result<Self, ColumnError> {
        i64::from_value(value).map(Cents)
    }
}

#[megh::entity(table = "notes", keys = ["tenant_id", "note_id"], timestamps, soft_delete)]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Validate)]
struct Note {
    tenant_id: Uuid,
    note_id: Uuid,
    #[validate(length(min = 1))]
    body: String,
    price: Cents,
}

/// Written as encrypted bytes, read back decrypted: the type owns its serialization.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
struct Secret(String);

const PAD: &[u8] = b"megh-test-key";

fn encrypt(bytes: &[u8]) -> Vec<u8> {
    bytes.iter().zip(PAD.iter().cycle()).map(|(byte, pad)| byte ^ pad).collect()
}

impl Column for Secret {
    const KIND: Kind = Kind::Bytes;
    fn to_value(&self) -> Value {
        encrypt(self.0.as_bytes()).into()
    }
    fn from_value(value: Value) -> Result<Self, ColumnError> {
        String::from_utf8(encrypt(&Vec::<u8>::from_value(value)?)).map(Secret).map_err(|error| ColumnError::Invalid(error.to_string()))
    }
}

#[megh::entity(table = "vaults", keys = ["tenant_id", "id"], timestamps, soft_delete)]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Validate)]
struct Vault {
    tenant_id: Uuid,
    #[serde(default = "Uuid::now_v7")]
    id: Uuid,
    label: String,
    token: Secret,
}

#[megh::entity(table = "wallets", keys = ["tenant_id", "id"], timestamps, soft_delete)]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Validate)]
struct Wallet {
    tenant_id: Uuid,
    #[serde(default = "Uuid::now_v7")]
    id: Uuid,
    owner: Uuid,
    amount: Decimal,
    rate: f64,
    count: i32,
    total: i64,
    active: bool,
    opened: NaiveDate,
    seen: DateTime<Utc>,
    nickname: Option<String>,
    discount: Option<Decimal>,
    opens: NaiveTime,
    members: Vec<Uuid>,
    labels: Vec<String>,
}

/// A database enum type, named and cased as sqlx names it.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, megh::Column)]
#[sqlx(type_name = "color", rename_all = "lowercase")]
enum Color {
    Red,
    DarkBlue,
}

#[megh::entity(table = "paints", keys = ["tenant_id", "id"], timestamps, soft_delete)]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Validate)]
struct Paint {
    tenant_id: Uuid,
    #[serde(default = "Uuid::now_v7")]
    id: Uuid,
    shade: Color,
}

#[megh::entity(table = "memos", keys = ["tenant_id", "id"], timestamps, soft_delete, audit = per_table)]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Validate)]
struct Memo {
    tenant_id: Uuid,
    id: Uuid,
    body: String,
}

#[megh::entity(table = "slips", keys = ["tenant_id", "id"], timestamps, soft_delete, audit = "slip_log")]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Validate)]
struct Slip {
    tenant_id: Uuid,
    id: Uuid,
    body: String,
}

#[megh::entity(table = "tags", keys = ["tenant_id", "id"], timestamps)]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Validate)]
struct Tag {
    tenant_id: Uuid,
    id: Uuid,
    body: String,
}

#[megh::entity(table = "orphans", keys = ["tenant_id", "id"], timestamps, soft_delete, audit = "missing_audit")]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Validate)]
struct Orphan {
    tenant_id: Uuid,
    id: Uuid,
    body: String,
}

fn make<T: serde::de::DeserializeOwned>(value: Json) -> T {
    serde_json::from_value(value).unwrap()
}

fn client(tenant: Uuid, first: &str, extra: Json) -> Client {
    let mut value = json!({
        "tenant_id": tenant, "id": Uuid::now_v7(), "first_name": first, "last_name": "Lovelace", "name": first,
        "gender": "female", "birth_date": "1815-12-10", "email": "ada@example.com",
        "attributes": {"vip": true, "tier": "gold"},
    });
    value.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
    make(value)
}

fn key(tenant_id: Uuid, id: Uuid) -> Key {
    Key { tenant_id, id }
}

fn changes(tenant: Uuid, id: Uuid, patch: Json) -> Changes {
    Changes { key: key(tenant, id), changes: patch.as_object().unwrap().clone() }
}

fn request(tenant_id: Uuid, filter: Option<&str>, size: u16, page_token: Option<String>) -> ListRequest {
    ListRequest {
        tenant_id,
        filter: filter.map(|text| parse_filter(text).unwrap()),
        page: PageRequest { page_size: Some(size), page_token },
    }
}

async fn names<DB: Backend>(db: &Pool<DB>, tenant: Uuid, filter: &str) -> Vec<String> {
    let page = crud::list::<Client, _>(db, &request(tenant, Some(filter), 100, None)).await.unwrap();
    let mut names: Vec<String> = page.items.iter().map(|client| client.first_name.as_ref().to_string()).collect();
    names.sort();
    names
}

/// Reads rows with the library's own reader, whatever the database.
async fn read<DB: Backend>(db: &Pool<DB>, query: SelectStatement, columns: &[(&'static str, Kind)]) -> Vec<Vec<Value>> {
    let columns: Vec<ColumnMeta> = columns
        .iter()
        .map(|(name, kind)| ColumnMeta { name, kind: *kind, type_name: None, is_primary_key: false })
        .collect();
    let mut connection = db.acquire().await.unwrap();
    DB::fetch(&mut connection, DB::build(&query), &columns).await.unwrap()
}

async fn count<DB: Backend>(db: &Pool<DB>, table: &str, condition: Condition) -> i64 {
    let query = Query::select().expr_as(Func::count(Expr::col(Asterisk)), Alias::new("n")).from(Alias::new(table)).cond_where(condition).to_owned();
    i64::from_value(read(db, query, &[("n", Kind::Int)]).await.remove(0).remove(0)).unwrap()
}

fn marked_deleted() -> Condition {
    Condition::all().add(Expr::col(Alias::new("deleted_at")).is_not_null())
}

async fn pause() {
    tokio::time::sleep(Duration::from_millis(20)).await;
}

async fn audit<DB: Backend>(db: &Pool<DB>, table: &str, entity: &str) -> Vec<(String, Option<String>, bool, bool, String)> {
    let columns = ["action", "actor_id", "old_data", "new_data", "record_id"];
    let query = Query::select()
        .columns(columns.map(Alias::new))
        .from(Alias::new(table))
        .and_where(Expr::col(Alias::new("table_name")).eq(entity))
        .order_by(Alias::new("id"), Order::Asc)
        .to_owned();
    let rows = read(db, query, &columns.map(|name| (name, Kind::Text))).await;
    rows.into_iter()
        .map(|row| {
            let [action, actor, old, new, record] = <[Value; 5]>::try_from(row).unwrap();
            let text = |value| Option::<String>::from_value(value).unwrap();
            (text(action).unwrap(), text(actor), text(old).is_none(), text(new).is_none(), text(record).unwrap())
        })
        .collect()
}

// --- Databases: SQLite in memory, PostgreSQL through the project's migrations; uuids are BLOB on SQLite ---

/// The column types of one database; SQLite has no uuid, decimal, time or list type, so they are BLOB and TEXT there.
struct Sql {
    uuid: &'static str,
    timestamp: &'static str,
    date: &'static str,
    decimal: &'static str,
    bytes: &'static str,
    float: &'static str,
    boolean: &'static str,
    time: &'static str,
    uuids: &'static str,
    texts: &'static str,
    color: &'static str,
    color_type: &'static str,
}

const SQLITE: Sql = Sql {
    uuid: "BLOB",
    timestamp: "TEXT",
    date: "TEXT",
    decimal: "TEXT",
    bytes: "BLOB",
    float: "REAL",
    boolean: "INTEGER",
    time: "TEXT",
    uuids: "TEXT",
    texts: "TEXT",
    color: "TEXT",
    color_type: "",
};

const POSTGRES: Sql = Sql {
    uuid: "UUID",
    timestamp: "TIMESTAMPTZ",
    date: "DATE",
    decimal: "NUMERIC(20,4)",
    bytes: "BYTEA",
    float: "DOUBLE PRECISION",
    boolean: "BOOLEAN",
    time: "TIME",
    uuids: "UUID[]",
    texts: "TEXT[]",
    color: "color",
    color_type: "CREATE TYPE color AS ENUM ('red', 'darkblue');",
};

fn audit_table(name: &str, sql: &Sql) -> String {
    let Sql { uuid, timestamp, .. } = sql;
    format!(
        "CREATE TABLE {name} (id {uuid} PRIMARY KEY, table_name TEXT NOT NULL, record_id TEXT NOT NULL, action TEXT NOT NULL,
         actor_id TEXT, old_data TEXT, new_data TEXT, created_at {timestamp});"
    )
}

fn test_tables(sql: &Sql) -> String {
    let Sql { uuid, timestamp, date, decimal, bytes, float, boolean, time, uuids, texts, color, color_type } = sql;
    let envelope = format!("created_at {timestamp} NOT NULL, updated_at {timestamp} NOT NULL, deleted_at {timestamp}");
    let simple = |name: &str| {
        format!("CREATE TABLE {name} (tenant_id {uuid} NOT NULL, id {uuid} NOT NULL, body TEXT NOT NULL, {envelope}, PRIMARY KEY (tenant_id, id));")
    };
    format!(
        "CREATE TABLE notes (tenant_id {uuid} NOT NULL, note_id {uuid} NOT NULL, body TEXT NOT NULL, price BIGINT NOT NULL, {envelope},
         PRIMARY KEY (tenant_id, note_id));
         CREATE TABLE vaults (tenant_id {uuid} NOT NULL, id {uuid} NOT NULL, label TEXT NOT NULL, token {bytes} NOT NULL, {envelope},
         PRIMARY KEY (tenant_id, id));
         {color_type}
         CREATE TABLE paints (tenant_id {uuid} NOT NULL, id {uuid} NOT NULL, shade {color} NOT NULL, {envelope}, PRIMARY KEY (tenant_id, id));
         CREATE TABLE wallets (tenant_id {uuid} NOT NULL, id {uuid} NOT NULL, owner {uuid} NOT NULL, amount {decimal} NOT NULL,
         rate {float} NOT NULL, count INTEGER NOT NULL, total BIGINT NOT NULL, active {boolean} NOT NULL, opened {date} NOT NULL,
         seen {timestamp} NOT NULL, nickname TEXT, discount {decimal}, opens {time} NOT NULL, members {uuids} NOT NULL,
         labels {texts} NOT NULL, {envelope}, PRIMARY KEY (tenant_id, id));
         CREATE TABLE tags (tenant_id {uuid} NOT NULL, id {uuid} NOT NULL, body TEXT NOT NULL, created_at {timestamp} NOT NULL,
         updated_at {timestamp} NOT NULL, PRIMARY KEY (tenant_id, id));
         {} {} {} {} {}",
        simple("memos"),
        simple("slips"),
        simple("orphans"),
        audit_table("memos_audit", sql),
        audit_table("slip_log", sql),
    )
}

async fn sqlite_db() -> (SqlitePool, [Uuid; 2]) {
    let pool = SqlitePoolOptions::new().max_connections(1).connect("sqlite::memory:").await.unwrap();
    let clients = "CREATE TABLE clients (tenant_id BLOB NOT NULL, id BLOB NOT NULL, first_name TEXT NOT NULL, last_name TEXT NOT NULL,
        name TEXT NOT NULL, role TEXT, email TEXT, home_phone TEXT, work_phone TEXT, mobile_phone TEXT,
        gender TEXT NOT NULL DEFAULT 'unknown', birth_date TEXT, address TEXT, attributes TEXT NOT NULL DEFAULT '{}',
        created_at TEXT NOT NULL, updated_at TEXT NOT NULL, deleted_at TEXT, moved_to BLOB, PRIMARY KEY (tenant_id, id));";
    let sql = format!("{clients} {} {}", audit_table("audit_entries", &SQLITE), test_tables(&SQLITE));
    sqlx::raw_sql(&sql).execute(&pool).await.unwrap();
    (pool, [Uuid::now_v7(), Uuid::now_v7()])
}

#[cfg(feature = "postgres")]
async fn postgres_db(pool: PgPool) -> (PgPool, [Uuid; 2]) {
    async fn tenant(pool: &PgPool) -> Uuid {
        sqlx::query_scalar("INSERT INTO tenants (name) VALUES ('Clinic') RETURNING id").fetch_one(pool).await.unwrap()
    }
    megh::migrate(&pool).await.unwrap();
    sqlx::raw_sql(&test_tables(&POSTGRES)).execute(&pool).await.unwrap();
    let tenants = [tenant(&pool).await, tenant(&pool).await];
    (pool, tenants)
}

/// Runs the scenario `$name` once on SQLite and once on PostgreSQL.
macro_rules! dual {
    ($name:ident) => {
        mod $name {
            #[tokio::test]
            async fn sqlite() {
                let (db, tenants) = super::sqlite_db().await;
                super::$name(&db, tenants).await;
            }

            #[cfg(feature = "postgres")]
            #[sqlx::test(migrations = false)]
            async fn postgres(pool: sqlx::PgPool) {
                let (db, tenants) = super::postgres_db(pool).await;
                super::$name(&db, tenants).await;
            }
        }
    };
}

// --- Create and get ---

async fn create_and_get<DB: Backend>(db: &Pool<DB>, [tenant, other]: [Uuid; 2]) {
    let before = Utc::now() - Span::seconds(1);
    let input = client(tenant, "Ada", json!({}));
    let created = crud::create(db, input.clone(), Some("a1")).await.unwrap();

    assert_ne!(created.id, input.id, "the id is generated, not the one sent");
    assert_eq!(created.first_name.as_ref(), "Ada");
    assert_eq!(created.contact.email.as_ref().unwrap().as_ref(), "ada@example.com");
    assert!(created.created_at >= before && created.created_at <= Utc::now() + Span::seconds(1));
    assert_eq!(created.updated_at, Some(created.created_at));
    assert!(created.deleted_at.is_none());

    let got = crud::get::<Client, _>(db, &key(tenant, created.id)).await.unwrap();
    assert_eq!(got, created);
    assert!(matches!(crud::get::<Client, _>(db, &key(other, created.id)).await, Err(ApiError::NotFound)));
    assert!(matches!(crud::get::<Client, _>(db, &key(tenant, Uuid::now_v7())).await, Err(ApiError::NotFound)));
}
dual!(create_and_get);

async fn create_ignores_supplied_timestamps<DB: Backend>(db: &Pool<DB>, [tenant, _]: [Uuid; 2]) {
    let stale = json!({"created_at": "2001-01-01T00:00:00Z", "updated_at": "2001-01-01T00:00:00Z", "deleted_at": "2001-01-01T00:00:00Z"});
    let created = crud::create(db, client(tenant, "Ada", stale), None).await.unwrap();
    assert!(created.created_at > Utc::now() - Span::minutes(1));
    assert!(created.updated_at.unwrap() > Utc::now() - Span::minutes(1));
    assert!(created.deleted_at.is_none());
    assert!(crud::get::<Client, _>(db, &key(tenant, created.id)).await.is_ok());
}
dual!(create_ignores_supplied_timestamps);

// --- Replace ---

async fn replace<DB: Backend>(db: &Pool<DB>, [tenant, _]: [Uuid; 2]) {
    let created = crud::create(db, client(tenant, "Ada", json!({})), None).await.unwrap();
    pause().await;
    let supplied = json!({"id": created.id, "created_at": "2001-01-01T00:00:00Z", "updated_at": "2001-01-01T00:00:00Z", "deleted_at": "2001-01-01T00:00:00Z"});
    let replaced = crud::replace(db, client(tenant, "Augusta", supplied), Some("a3")).await.unwrap();

    assert_eq!(replaced.id, created.id);
    assert_eq!(replaced.first_name.as_ref(), "Augusta");
    assert_eq!(replaced.created_at, created.created_at);
    assert!(replaced.updated_at.unwrap() > created.updated_at.unwrap(), "the time of the update, not the one supplied");
    assert!(replaced.deleted_at.is_none());
    assert_eq!(crud::get::<Client, _>(db, &key(tenant, created.id)).await.unwrap(), replaced);

    let missing = client(tenant, "Nobody", json!({}));
    assert!(matches!(crud::replace(db, missing, None).await, Err(ApiError::NotFound)));
}
dual!(replace);

// --- Patch: only the named columns change ---

async fn patch<DB: Backend>(db: &Pool<DB>, [tenant, _]: [Uuid; 2]) {
    let created = crud::create(db, client(tenant, "Ada", json!({})), None).await.unwrap();
    pause().await;

    let patched = crud::patch::<Client, _>(db, changes(tenant, created.id, json!({"role": "mathematician"})), Some("a2")).await.unwrap();
    let expected = Client { role: Name::try_new("mathematician").ok(), updated_at: patched.updated_at, ..created.clone() };
    assert_eq!(patched, expected);
    assert!(patched.updated_at.unwrap() > created.updated_at.unwrap());
    assert_eq!(crud::get::<Client, _>(db, &key(tenant, created.id)).await.unwrap(), patched);

    let reset = crud::patch::<Client, _>(db, changes(tenant, created.id, json!({"role": null})), None).await.unwrap();
    assert!(reset.role.is_none());

    let replaced = crud::patch::<Client, _>(db, changes(tenant, created.id, json!({"attributes": {"x": 1}})), None).await.unwrap();
    assert_eq!(serde_json::to_value(&replaced.attributes).unwrap(), json!({"x": 1}), "a column is replaced, never merged");

    let unchanged = crud::get::<Client, _>(db, &key(tenant, created.id)).await.unwrap();
    for bad in [json!({"nope": 1}), json!({"first_name": ""}), json!({"gender": "alien"}), json!({"email": "not-an-email"})] {
        let result = crud::patch::<Client, _>(db, changes(tenant, created.id, bad), None).await;
        assert!(matches!(result, Err(ApiError::BadRequest(_))));
        assert_eq!(crud::get::<Client, _>(db, &key(tenant, created.id)).await.unwrap(), unchanged);
    }
    let missing = crud::patch::<Client, _>(db, changes(tenant, Uuid::now_v7(), json!({"role": "x"})), None).await;
    assert!(matches!(missing, Err(ApiError::NotFound)));
}
dual!(patch);

// --- Soft delete ---

async fn soft_delete<DB: Backend>(db: &Pool<DB>, [tenant, _]: [Uuid; 2]) {
    let created = crud::create(db, client(tenant, "Ada", json!({"role": "mathematician"})), None).await.unwrap();
    let id = created.id;

    let deleted = crud::delete::<Client, _>(db, &key(tenant, id), Some("a4")).await.unwrap();
    assert_eq!(deleted.id, id);
    assert!(deleted.deleted_at.is_some());

    assert!(matches!(crud::get::<Client, _>(db, &key(tenant, id)).await, Err(ApiError::NotFound)));
    assert!(matches!(crud::delete::<Client, _>(db, &key(tenant, id), None).await, Err(ApiError::NotFound)));
    assert!(matches!(crud::patch::<Client, _>(db, changes(tenant, id, json!({"role": "x"})), None).await, Err(ApiError::NotFound)));
    assert!(matches!(crud::replace(db, client(tenant, "Ada", json!({"id": id})), None).await, Err(ApiError::NotFound)));
    assert!(crud::list::<Client, _>(db, &request(tenant, None, 10, None)).await.unwrap().items.is_empty());
    let selected = crud::select::<Client, _>(db, |query| {
        query.and_where(Expr::col(Alias::new("role")).eq("mathematician"));
    })
    .await
    .unwrap();
    assert!(selected.is_empty());
    assert_eq!(count(db, "clients", Condition::all()).await, 1, "the row stays");
    assert_eq!(count(db, "clients", marked_deleted()).await, 1, "marked as deleted");
}
dual!(soft_delete);

async fn delete_without_soft_delete_removes_the_row<DB: Backend>(db: &Pool<DB>, [tenant, _]: [Uuid; 2]) {
    let tag: Tag = make(json!({"tenant_id": tenant, "id": Uuid::now_v7(), "body": "t"}));
    let created = crud::create(db, tag, None).await.unwrap();

    let deleted = crud::delete::<Tag, _>(db, &key(tenant, created.id), None).await.unwrap();
    assert_eq!(deleted.id, created.id);
    assert!(matches!(crud::get::<Tag, _>(db, &key(tenant, created.id)).await, Err(ApiError::NotFound)));
    assert!(matches!(crud::delete::<Tag, _>(db, &key(tenant, created.id), None).await, Err(ApiError::NotFound)));
    assert_eq!(count(db, "tags", Condition::all()).await, 0, "the row is gone, not marked");
}
dual!(delete_without_soft_delete_removes_the_row);

// --- Validation, an explicit key name and a type the library does not know ---

async fn validation_and_custom_types<DB: Backend>(db: &Pool<DB>, [tenant, _]: [Uuid; 2]) {
    let note: Note = make(json!({"tenant_id": tenant, "note_id": Uuid::now_v7(), "body": "first", "price": 1999}));
    let created = crud::create(db, note.clone(), None).await.unwrap();
    let note_id = created.note_id;
    assert_eq!(created.price, Cents(1999));
    assert_eq!(crud::get::<Note, _>(db, &key(tenant, note_id)).await.unwrap(), created);

    let patched = crud::patch::<Note, _>(db, changes(tenant, note_id, json!({"price": 2500})), None).await.unwrap();
    assert_eq!(patched.price, Cents(2500));
    assert_eq!(patched.body, "first");

    let empty: Note = make(json!({"tenant_id": tenant, "note_id": Uuid::now_v7(), "body": "", "price": 1}));
    assert!(matches!(crud::create(db, empty.clone(), None).await, Err(ApiError::BadRequest(_))));
    assert!(matches!(crud::replace(db, Note { note_id, ..empty }, None).await, Err(ApiError::BadRequest(_))));
    assert_eq!(crud::select::<Note, _>(db, |_| {}).await.unwrap().len(), 1);
    assert_eq!(crud::get::<Note, _>(db, &key(tenant, note_id)).await.unwrap(), patched);

    assert!(crud::delete::<Note, _>(db, &key(tenant, note_id), None).await.unwrap().deleted_at.is_some());
}
dual!(validation_and_custom_types);

// --- The system decides: the id and the timestamps are never the ones an entity carries ---

async fn create_generates_the_id<DB: Backend>(db: &Pool<DB>, [tenant, _]: [Uuid; 2]) {
    let sent = Uuid::now_v7();
    let created = crud::create(db, client(tenant, "Ada", json!({"id": sent})), None).await.unwrap();
    assert_ne!(created.id, sent);
    assert_eq!(created.id.get_version_num(), 7);
    assert!(matches!(crud::get::<Client, _>(db, &key(tenant, sent)).await, Err(ApiError::NotFound)));
    assert!(crud::get::<Client, _>(db, &key(tenant, created.id)).await.is_ok());

    let note: Note = make(json!({"tenant_id": tenant, "note_id": sent, "body": "n", "price": 1}));
    let created = crud::create(db, note, None).await.unwrap();
    assert_ne!(created.note_id, sent, "the last key is the id, whatever its name");
    assert!(matches!(crud::get::<Note, _>(db, &key(tenant, sent)).await, Err(ApiError::NotFound)));
}
dual!(create_generates_the_id);

async fn patch_ignores_system_fields<DB: Backend>(db: &Pool<DB>, [tenant, _]: [Uuid; 2]) {
    let created = crud::create(db, client(tenant, "Ada", json!({})), None).await.unwrap();
    pause().await;
    let stale = json!({"role": "x", "created_at": "2001-01-01T00:00:00Z", "updated_at": "2001-01-01T00:00:00Z", "deleted_at": "2001-01-01T00:00:00Z"});
    let patched = crud::patch::<Client, _>(db, changes(tenant, created.id, stale), None).await.unwrap();
    assert_eq!(patched.role.as_ref().map(|role| role.as_ref()), Some("x"));
    assert_eq!(patched.created_at, created.created_at);
    assert!(patched.updated_at.unwrap() > created.updated_at.unwrap());
    assert!(patched.deleted_at.is_none());
    assert!(crud::get::<Client, _>(db, &key(tenant, created.id)).await.is_ok(), "not deleted by a supplied deleted_at");
}
dual!(patch_ignores_system_fields);

// --- Several keys: a row is the pair (tenant, id) ---

async fn composite_key<DB: Backend>(db: &Pool<DB>, [first, second]: [Uuid; 2]) {
    let note = |tenant: Uuid| -> Note { make(json!({"tenant_id": tenant, "note_id": Uuid::now_v7(), "body": "n", "price": 1})) };
    let mine = crud::create(db, note(first), None).await.unwrap();
    let theirs = crud::create(db, note(second), None).await.unwrap();

    assert_eq!(crud::get::<Note, _>(db, &key(first, mine.note_id)).await.unwrap(), mine);
    assert_eq!(crud::get::<Note, _>(db, &key(second, theirs.note_id)).await.unwrap(), theirs);
    assert!(matches!(crud::get::<Note, _>(db, &key(second, mine.note_id)).await, Err(ApiError::NotFound)));
    assert!(matches!(crud::get::<Note, _>(db, &key(first, theirs.note_id)).await, Err(ApiError::NotFound)));

    crud::delete::<Note, _>(db, &key(first, mine.note_id), None).await.unwrap();
    assert!(crud::get::<Note, _>(db, &key(second, theirs.note_id)).await.is_ok(), "the other tenant's row is untouched");
    assert!(matches!(crud::delete::<Note, _>(db, &key(second, mine.note_id), None).await, Err(ApiError::NotFound)));
}
dual!(composite_key);

// --- The common column types, lists, a database enum and a type with its own serialization ---

async fn common_types<DB: Backend>(db: &Pool<DB>, [tenant, _]: [Uuid; 2]) {
    let (owner, other) = (Uuid::now_v7(), Uuid::now_v7());
    let seen = Utc.with_ymd_and_hms(2024, 5, 6, 7, 8, 9).unwrap() + Span::microseconds(123_456);
    let wallet: Wallet = make(json!({
        "tenant_id": tenant, "owner": owner, "amount": "12.50", "rate": 0.25, "count": 3, "total": 9_000_000_000i64,
        "active": true, "opened": "2021-01-02", "seen": seen, "nickname": null, "discount": null,
        "opens": "08:30:00", "members": [owner, other], "labels": ["a", "b"],
    }));
    let created = crud::create(db, wallet, None).await.unwrap();
    assert_eq!(created.owner, owner);
    assert_eq!(created.amount, Decimal::new(1250, 2));
    assert_eq!((created.rate, created.count, created.total, created.active), (0.25, 3, 9_000_000_000, true));
    assert_eq!(created.opened, NaiveDate::from_ymd_opt(2021, 1, 2).unwrap());
    assert_eq!(created.seen, seen);
    assert!(created.nickname.is_none() && created.discount.is_none());
    assert_eq!(
        (created.opens, created.members.clone(), created.labels.clone()),
        (NaiveTime::from_hms_opt(8, 30, 0).unwrap(), vec![owner, other], vec!["a".to_string(), "b".to_string()])
    );
    assert_eq!(crud::get::<Wallet, _>(db, &key(tenant, created.id)).await.unwrap(), created);

    let patch = json!({
        "owner": other, "amount": "99.95", "rate": 1.5, "count": -7, "total": 1, "active": false, "opened": "2022-03-04",
        "seen": "2020-02-03T04:05:06Z", "nickname": "w", "discount": "0.15",
        "opens": "17:45:00", "members": [], "labels": ["x"],
    });
    let patched = crud::patch::<Wallet, _>(db, changes(tenant, created.id, patch), None).await.unwrap();
    assert_eq!(patched.owner, other);
    assert_eq!(patched.amount, Decimal::new(9995, 2));
    assert_eq!((patched.rate, patched.count, patched.total, patched.active), (1.5, -7, 1, false));
    assert_eq!(patched.opened, NaiveDate::from_ymd_opt(2022, 3, 4).unwrap());
    assert_eq!(patched.seen, Utc.with_ymd_and_hms(2020, 2, 3, 4, 5, 6).unwrap());
    assert_eq!(patched.nickname.as_deref(), Some("w"));
    assert_eq!(patched.discount, Some(Decimal::new(15, 2)));
    assert_eq!(
        (patched.opens, patched.members.clone(), patched.labels.clone()),
        (NaiveTime::from_hms_opt(17, 45, 0).unwrap(), vec![], vec!["x".to_string()])
    );
    assert_eq!(crud::get::<Wallet, _>(db, &key(tenant, created.id)).await.unwrap(), patched);
}
dual!(common_types);

async fn database_enum_named_as_sqlx_names_it<DB: Backend>(db: &Pool<DB>, [tenant, _]: [Uuid; 2]) {
    let paint: Paint = make(json!({"tenant_id": tenant, "shade": "DarkBlue"}));
    let created = crud::create(db, paint, None).await.unwrap();
    assert_eq!(created.shade, Color::DarkBlue);
    assert_eq!(crud::get::<Paint, _>(db, &key(tenant, created.id)).await.unwrap(), created);

    let patched = crud::patch::<Paint, _>(db, changes(tenant, created.id, json!({"shade": "Red"})), None).await.unwrap();
    assert_eq!(patched.shade, Color::Red);
    let red = crud::list::<Paint, _>(db, &request(tenant, Some(r#"shade = "Red""#), 10, None)).await.unwrap();
    assert_eq!(red.items.len(), 1, "a filter value is cast to the database type");
    let bad = crud::patch::<Paint, _>(db, changes(tenant, created.id, json!({"shade": "Purple"})), None).await;
    assert!(matches!(bad, Err(ApiError::BadRequest(_))));
}
dual!(database_enum_named_as_sqlx_names_it);

async fn stored_token<DB: Backend>(db: &Pool<DB>) -> Vec<u8> {
    let query = Query::select().column(Alias::new("token")).from(Alias::new("vaults")).to_owned();
    Vec::<u8>::from_value(read(db, query, &[("token", Kind::Bytes)]).await.remove(0).remove(0)).unwrap()
}

async fn secret_is_stored_encrypted<DB: Backend>(db: &Pool<DB>, [tenant, _]: [Uuid; 2]) {
    let vault: Vault = make(json!({"tenant_id": tenant, "label": "prod", "token": "s3cr3t-token"}));
    let created = crud::create(db, vault, None).await.unwrap();
    assert_eq!(created.token, Secret("s3cr3t-token".into()), "read back decrypted");
    assert_eq!(crud::get::<Vault, _>(db, &key(tenant, created.id)).await.unwrap(), created);
    assert_eq!(stored_token(db).await, encrypt(b"s3cr3t-token"));
    assert!(!String::from_utf8_lossy(&stored_token(db).await).contains("s3cr3t"), "the plain text is not in the database");

    let rotated = crud::patch::<Vault, _>(db, changes(tenant, created.id, json!({"token": "rotated"})), None).await.unwrap();
    assert_eq!(rotated.token, Secret("rotated".into()));
    assert_eq!(stored_token(db).await, encrypt(b"rotated"), "a patched value goes through the type too");

    let replacement: Vault = make(json!({"tenant_id": tenant, "id": created.id, "label": "prod", "token": "replaced"}));
    crud::replace(db, replacement, None).await.unwrap();
    assert_eq!(stored_token(db).await, encrypt(b"replaced"));
}
dual!(secret_is_stored_encrypted);

// --- List: AIP-160 filter, tenant scope, AIP-158 pagination ---

async fn seed<DB: Backend>(db: &Pool<DB>, clients: Vec<Client>) -> Vec<Client> {
    join_all(clients.into_iter().map(|client| crud::create(db, client, None))).await.into_iter().map(Result::unwrap).collect()
}

async fn list_filter<DB: Backend>(db: &Pool<DB>, [tenant, other]: [Uuid; 2]) {
    seed(db, vec![
        client(tenant, "Ada", json!({"role": "mathematician", "birth_date": "1815-12-10", "attributes": {"vip": true, "tier": "gold"}})),
        client(tenant, "Grace", json!({"role": "admiral", "birth_date": "1906-12-09", "attributes": {"vip": false, "tier": "silver"}})),
        client(tenant, "Alan", json!({"role": "mathematician", "birth_date": "1912-06-23", "attributes": {"tier": "gold"}})),
    ])
    .await;
    seed(db, vec![client(other, "Eve", json!({"role": "mathematician"}))]).await;

    assert_eq!(names(db, tenant, r#"role = "mathematician""#).await, ["Ada", "Alan"]);
    assert_eq!(names(db, tenant, r#"first_name = "Grace""#).await, ["Grace"]);
    assert_eq!(names(db, tenant, r#"birth_date < "1900-01-01""#).await, ["Ada"]);
    assert_eq!(names(db, tenant, "vip = true").await, ["Ada"]);
    assert_eq!(names(db, tenant, r#"tier = "gold""#).await, ["Ada", "Alan"]);
    assert_eq!(names(db, tenant, r#"tier = "silver""#).await, ["Grace"]);
    assert_eq!(names(db, tenant, r#"role = "mathematician" AND tier = "gold" AND NOT first_name = "Alan""#).await, ["Ada"]);
    assert_eq!(names(db, tenant, r#"role = "admiral" OR first_name = "Ada""#).await, ["Ada", "Grace"]);

    let everyone = crud::list::<Client, _>(db, &request(tenant, None, 100, None)).await.unwrap();
    assert_eq!(everyone.items.len(), 3);
}
dual!(list_filter);

async fn list_has<DB: Backend>(db: &Pool<DB>, [tenant, _]: [Uuid; 2]) {
    let (first, second) = (Uuid::now_v7(), Uuid::now_v7());
    let wallet = |members: Json, labels: Json| -> Wallet {
        make(json!({
            "tenant_id": tenant, "owner": first, "amount": "1", "rate": 1.0, "count": 1, "total": 1, "active": true,
            "opened": "2021-01-02", "seen": "2020-02-03T04:05:06Z", "opens": "08:30:00", "members": members, "labels": labels,
        }))
    };
    crud::create(db, wallet(json!([first, second]), json!(["a", "b"])), None).await.unwrap();
    crud::create(db, wallet(json!([second]), json!(["b"])), None).await.unwrap();
    let count = |filter: String| {
        let request = request(tenant, Some(&filter), 10, None);
        async move { crud::list::<Wallet, _>(db, &request).await.unwrap().items.len() }
    };
    assert_eq!(count(format!(r#"members:"{first}""#)).await, 1, "an element of a list of uuids");
    assert_eq!(count(format!(r#"members:"{second}""#)).await, 2);
    assert_eq!(count(r#"labels:"a""#.into()).await, 1, "an element of a list of text");
    assert_eq!(count(r#"labels:"c""#.into()).await, 0);
    assert!(crud::list::<Wallet, _>(db, &request(tenant, Some(r#"count:1"#), 10, None)).await.is_err(), "a column that is not a list has no elements");

    seed(db, vec![client(tenant, "Ada", json!({"attributes": {"tags": ["x", "y"]}})), client(tenant, "Grace", json!({"attributes": {"tags": ["y"]}}))]).await;
    assert_eq!(names(db, tenant, r#"tags:"x""#).await, ["Ada"], "an element of a JSON array in attributes");
    assert_eq!(names(db, tenant, r#"tags:"y""#).await, ["Ada", "Grace"]);
}
dual!(list_has);

async fn filter_keys_are_not_sql<DB: Backend>(db: &Pool<DB>, [tenant, _]: [Uuid; 2]) {
    seed(db, vec![client(tenant, "Ada", json!({})), client(tenant, "Grace", json!({}))]).await;
    if let Ok(filter) = parse_filter("a'b = 1") {
        let page = crud::list::<Client, _>(db, &ListRequest { tenant_id: tenant, filter: Some(filter), page: PageRequest { page_size: Some(10), page_token: None } }).await;
        assert!(page.map_or(true, |page| page.items.is_empty()));
    }
    assert_eq!(crud::list::<Client, _>(db, &request(tenant, None, 10, None)).await.unwrap().items.len(), 2);
}
dual!(filter_keys_are_not_sql);

async fn pagination<DB: Backend>(db: &Pool<DB>, [tenant, _]: [Uuid; 2]) {
    let all = seed(db, ["A", "B", "C", "D", "E"].map(|first| client(tenant, first, json!({}))).to_vec()).await;
    let mut ids: Vec<Uuid> = all.iter().map(|client| client.id).collect();
    ids.sort();

    let first = crud::list::<Client, _>(db, &request(tenant, None, 2, None)).await.unwrap();
    let second = crud::list::<Client, _>(db, &request(tenant, None, 2, first.next_page_token.clone())).await.unwrap();
    let third = crud::list::<Client, _>(db, &request(tenant, None, 2, second.next_page_token.clone())).await.unwrap();

    assert_eq!([first.items.len(), second.items.len(), third.items.len()], [2, 2, 1]);
    assert!(first.next_page_token.is_some() && second.next_page_token.is_some() && third.next_page_token.is_none());
    let paged: Vec<Uuid> = [first.items, second.items, third.items].concat().iter().map(|client| client.id).collect();
    assert_eq!(paged, ids);

    let bad = crud::list::<Client, _>(db, &request(tenant, None, 2, Some("garbage".into()))).await;
    assert!(matches!(bad, Err(ApiError::BadRequest(_))));
}
dual!(pagination);

// --- Audit: written in the write's transaction, per the table's policy ---

async fn audit_trail<DB: Backend>(db: &Pool<DB>, [tenant, _]: [Uuid; 2]) {
    let created = crud::create(db, client(tenant, "Ada", json!({})), Some("a1")).await.unwrap();
    let id = created.id;
    crud::patch::<Client, _>(db, changes(tenant, id, json!({"role": "x"})), Some("a2")).await.unwrap();
    assert!(crud::patch::<Client, _>(db, changes(tenant, id, json!({"first_name": ""})), Some("bad")).await.is_err());
    crud::replace(db, client(tenant, "Augusta", json!({"id": id})), Some("a3")).await.unwrap();
    crud::delete::<Client, _>(db, &key(tenant, id), Some("a4")).await.unwrap();

    let rows = audit(db, "audit_entries", "clients").await;
    let actions: Vec<&str> = rows.iter().map(|row| row.0.as_str()).collect();
    let actors: Vec<Option<&str>> = rows.iter().map(|row| row.1.as_deref()).collect();
    assert_eq!(actions, ["INSERT", "UPDATE", "UPDATE", "DELETE"]);
    assert_eq!(actors, [Some("a1"), Some("a2"), Some("a3"), Some("a4")]);
    assert!(rows.iter().all(|row| row.4 == id.to_string()));
    assert!(rows[0].2 && !rows[0].3, "an insert has no old row");
    assert!(!rows[3].2 && rows[3].3, "a delete has no new row");
}
dual!(audit_trail);

async fn audit_policies<DB: Backend>(db: &Pool<DB>, [tenant, _]: [Uuid; 2]) {
    let note: Note = make(json!({"tenant_id": tenant, "note_id": Uuid::now_v7(), "body": "n", "price": 5}));
    let memo: Memo = make(json!({"tenant_id": tenant, "id": Uuid::now_v7(), "body": "m"}));
    let slip: Slip = make(json!({"tenant_id": tenant, "id": Uuid::now_v7(), "body": "s"}));
    crud::create(db, note, Some("a")).await.unwrap();
    crud::create(db, memo, Some("a")).await.unwrap();
    crud::create(db, slip, Some("a")).await.unwrap();

    assert!(audit(db, "audit_entries", "notes").await.is_empty(), "disabled by default, even with an actor");
    assert_eq!(audit(db, "memos_audit", "memos").await.len(), 1, "per table: <table>_audit");
    assert!(audit(db, "audit_entries", "memos").await.is_empty());
    assert_eq!(audit(db, "slip_log", "slips").await.len(), 1, "dedicated: the named table");
}
dual!(audit_policies);

async fn audit_failure_undoes_the_write<DB: Backend>(db: &Pool<DB>, [tenant, _]: [Uuid; 2]) {
    let orphan: Orphan = make(json!({"tenant_id": tenant, "id": Uuid::now_v7(), "body": "o"}));
    assert!(crud::create(db, orphan, Some("a")).await.is_err());
    assert!(crud::select::<Orphan, _>(db, |_| {}).await.unwrap().is_empty());
}
dual!(audit_failure_undoes_the_write);

// --- Inside a caller's transaction ---

#[tokio::test]
async fn crud_runs_in_the_callers_transaction() {
    let (pool, [tenant, _]) = sqlite_db().await;
    let mut transaction = pool.begin().await.unwrap();
    let created = crud::create(&mut *transaction, client(tenant, "Ada", json!({})), None).await.unwrap();
    assert!(crud::get::<Client, _>(&mut *transaction, &key(tenant, created.id)).await.is_ok());
    transaction.rollback().await.unwrap();
    assert!(matches!(crud::get::<Client, _>(&pool, &key(tenant, created.id)).await, Err(ApiError::NotFound)));
}

#[tokio::test]
async fn sqlite_stores_uuids_as_blob() {
    let (pool, [tenant, _]) = sqlite_db().await;
    crud::create(&pool, client(tenant, "Ada", json!({})), None).await.unwrap();
    let kind: String = sqlx::query_scalar("SELECT typeof(id) FROM clients").fetch_one(&pool).await.unwrap();
    assert_eq!(kind, "blob");
}
