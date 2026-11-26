//! The database megh runs on, chosen at start-up from its URL: PostgreSQL or SQLite.
//!
//! Statements are built as sqlx builds them (`Db::query`, `Db::query_as`, `Db::query_scalar`, `.bind`, `.fetch_one(&pool)`)
//! and written once in standard SQL; a driver whose SQL differs gets its own text (`.postgres(…)`, `.sqlite(…)`). Values
//! are bound and rows read through each type's own `sqlx::Type`.

use std::marker::PhantomData;

use sqlx::error::BoxDynError;

#[cfg(feature = "axum")]
use crate::auth::crud;
#[cfg(feature = "axum")]
use crate::auth::entity::{Table, Where};
#[cfg(feature = "axum")]
use crate::auth::rest::{ApiResult, Changes, Key, ListRequest, Page};

/// The current time in SQLite SQL, written as sqlx writes a `DateTime<Utc>` (RFC 3339), so the two compare as text:
/// `concat!("UPDATE … SET updated_at = ", sqlite_now!(), " …")`.
#[macro_export]
#[doc(hidden)]
macro_rules! sqlite_now {
    () => {
        "strftime('%Y-%m-%dT%H:%M:%f+00:00')"
    };
}

/// A pool of connections to one database.
#[derive(Debug, Clone)]
pub enum Db {
    #[cfg(feature = "postgres")]
    Postgres(sqlx::PgPool),
    #[cfg(feature = "sqlite")]
    Sqlite(sqlx::SqlitePool),
}

/// A transaction.
#[derive(Debug)]
pub enum Tx {
    #[cfg(feature = "postgres")]
    Postgres(sqlx::Transaction<'static, sqlx::Postgres>),
    #[cfg(feature = "sqlite")]
    Sqlite(sqlx::Transaction<'static, sqlx::Sqlite>),
}

/// The transaction a group of statements runs in: one of its own, committed with them, or the caller's, which the
/// caller commits.
pub enum Scope<'e> {
    Own(Tx),
    Within(&'e mut Tx),
}

impl std::ops::Deref for Scope<'_> {
    type Target = Tx;
    fn deref(&self) -> &Tx {
        match self {
            Scope::Own(tx) => tx,
            Scope::Within(tx) => tx,
        }
    }
}

impl std::ops::DerefMut for Scope<'_> {
    fn deref_mut(&mut self) -> &mut Tx {
        match self {
            Scope::Own(tx) => tx,
            Scope::Within(tx) => tx,
        }
    }
}

impl Scope<'_> {
    /// Commits a transaction of its own; the caller's is left to the caller.
    pub async fn commit(self) -> Result<(), sqlx::Error> {
        match self {
            Scope::Own(tx) => tx.commit().await,
            Scope::Within(_) => Ok(()),
        }
    }
}

impl Db {
    /// The driver is picked from the URL's scheme: `postgres://` or `postgresql://`, `sqlite:`.
    pub async fn connect(url: &str) -> Result<Db, sqlx::Error> {
        match url.split(':').next() {
            #[cfg(feature = "postgres")]
            Some("postgres" | "postgresql") => sqlx::PgPool::connect(url).await.map(Db::Postgres),
            #[cfg(feature = "sqlite")]
            Some("sqlite") => sqlx::SqlitePool::connect(url).await.map(Db::Sqlite),
            _ => Err(sqlx::Error::Configuration(format!("no driver for {url}").into())),
        }
    }

    /// megh's tables, from the migration set of the driver (`migrations/postgres`, `migrations/sqlite`). Each script is
    /// idempotent, so a database shared with megh-go keeps its tables.
    pub async fn migrate(&self) -> Result<(), sqlx::Error> {
        match self {
            #[cfg(feature = "postgres")]
            Db::Postgres(pool) => run_scripts(pool, &migrations!("postgres")).await,
            #[cfg(feature = "sqlite")]
            Db::Sqlite(pool) => run_scripts(pool, &migrations!("sqlite")).await,
        }
    }

    /// Closes every connection; later statements fail.
    pub async fn close(&self) {
        match self {
            #[cfg(feature = "postgres")]
            Db::Postgres(pool) => pool.close().await,
            #[cfg(feature = "sqlite")]
            Db::Sqlite(pool) => pool.close().await,
        }
    }

    /// A transaction. On SQLite it takes the write lock when it starts, so the rows it reads stay as read until it ends.
    pub async fn begin(&self) -> Result<Tx, sqlx::Error> {
        match self {
            #[cfg(feature = "postgres")]
            Db::Postgres(pool) => pool.begin().await.map(Tx::Postgres),
            #[cfg(feature = "sqlite")]
            Db::Sqlite(pool) => pool.begin_with("BEGIN IMMEDIATE").await.map(Tx::Sqlite),
        }
    }
}

/// The scripts of one driver's migration set, in order.
macro_rules! migrations {
    ($driver:literal) => {
        [
            include_str!(concat!("../../migrations/", $driver, "/0001_auth.sql")),
            include_str!(concat!("../../migrations/", $driver, "/0002_audit.sql")),
            include_str!(concat!("../../migrations/", $driver, "/0003_tenant.sql")),
            include_str!(concat!("../../migrations/", $driver, "/0004_booking.sql")),
            include_str!(concat!("../../migrations/", $driver, "/0005_billing.sql")),
            include_str!(concat!("../../migrations/", $driver, "/0006_products.sql")),
            include_str!(concat!("../../migrations/", $driver, "/0007_connections.sql")),
            include_str!(concat!("../../migrations/", $driver, "/0008_connection_spec.sql")),
            include_str!(concat!("../../migrations/", $driver, "/0009_messages.sql")),
            include_str!(concat!("../../migrations/", $driver, "/0010_attachments.sql")),
            include_str!(concat!("../../migrations/", $driver, "/0011_client_moved.sql")),
        ]
    };
}
use migrations;

/// Runs each script on `pool`, one after the other.
async fn run_scripts<DB: sqlx::Database>(pool: &sqlx::Pool<DB>, scripts: &[&'static str]) -> Result<(), sqlx::Error>
where
    for<'p> &'p sqlx::Pool<DB>: sqlx::Executor<'p, Database = DB>,
{
    use futures_util::{StreamExt, TryStreamExt};
    futures_util::stream::iter(scripts).then(|script| sqlx::raw_sql(script).execute(pool)).try_for_each(|_| async { Ok(()) }).await
}

impl Tx {
    pub async fn rollback(self) -> Result<(), sqlx::Error> {
        match self {
            #[cfg(feature = "postgres")]
            Tx::Postgres(tx) => tx.rollback().await,
            #[cfg(feature = "sqlite")]
            Tx::Sqlite(tx) => tx.rollback().await,
        }
    }

    pub async fn commit(self) -> Result<(), sqlx::Error> {
        match self {
            #[cfg(feature = "postgres")]
            Tx::Postgres(tx) => tx.commit().await,
            #[cfg(feature = "sqlite")]
            Tx::Sqlite(tx) => tx.commit().await,
        }
    }
}

/// A value bound on PostgreSQL: `sqlx::Type` there, or anything when megh is built without PostgreSQL.
#[cfg(feature = "postgres")]
pub trait OnPostgres<'q>: sqlx::Encode<'q, sqlx::Postgres> + sqlx::Type<sqlx::Postgres> {}
#[cfg(feature = "postgres")]
impl<'q, T: sqlx::Encode<'q, sqlx::Postgres> + sqlx::Type<sqlx::Postgres>> OnPostgres<'q> for T {}
#[cfg(not(feature = "postgres"))]
pub trait OnPostgres<'q> {}
#[cfg(not(feature = "postgres"))]
impl<T> OnPostgres<'_> for T {}

/// A value bound on SQLite: `sqlx::Type` there, or anything when megh is built without SQLite.
#[cfg(feature = "sqlite")]
pub trait OnSqlite<'q>: sqlx::Encode<'q, sqlx::Sqlite> + sqlx::Type<sqlx::Sqlite> {}
#[cfg(feature = "sqlite")]
impl<'q, T: sqlx::Encode<'q, sqlx::Sqlite> + sqlx::Type<sqlx::Sqlite>> OnSqlite<'q> for T {}
#[cfg(not(feature = "sqlite"))]
pub trait OnSqlite<'q> {}
#[cfg(not(feature = "sqlite"))]
impl<T> OnSqlite<'_> for T {}

/// A row read on PostgreSQL.
#[cfg(feature = "postgres")]
pub trait RowOnPostgres: for<'r> sqlx::FromRow<'r, sqlx::postgres::PgRow> {}
#[cfg(feature = "postgres")]
impl<T: for<'r> sqlx::FromRow<'r, sqlx::postgres::PgRow>> RowOnPostgres for T {}
#[cfg(not(feature = "postgres"))]
pub trait RowOnPostgres {}
#[cfg(not(feature = "postgres"))]
impl<T> RowOnPostgres for T {}

/// A row read on SQLite.
#[cfg(feature = "sqlite")]
pub trait RowOnSqlite: for<'r> sqlx::FromRow<'r, sqlx::sqlite::SqliteRow> {}
#[cfg(feature = "sqlite")]
impl<T: for<'r> sqlx::FromRow<'r, sqlx::sqlite::SqliteRow>> RowOnSqlite for T {}
#[cfg(not(feature = "sqlite"))]
pub trait RowOnSqlite {}
#[cfg(not(feature = "sqlite"))]
impl<T> RowOnSqlite for T {}

/// A value a statement binds, through its own `sqlx::Type` on every driver; owned, or borrowed for `'q`.
pub trait Param<'q>: Send + 'q {
    #[cfg(feature = "postgres")]
    fn add_postgres(self: Box<Self>, arguments: &mut sqlx::postgres::PgArguments) -> Result<(), BoxDynError>;
    #[cfg(feature = "sqlite")]
    fn add_sqlite(self: Box<Self>, arguments: &mut sqlx::sqlite::SqliteArguments<'q>) -> Result<(), BoxDynError>;
}

impl<'q, T: OnPostgres<'q> + OnSqlite<'q> + Send + 'q> Param<'q> for T {
    #[cfg(feature = "postgres")]
    fn add_postgres(self: Box<Self>, arguments: &mut sqlx::postgres::PgArguments) -> Result<(), BoxDynError> {
        sqlx::Arguments::add(arguments, *self)
    }
    #[cfg(feature = "sqlite")]
    fn add_sqlite(self: Box<Self>, arguments: &mut sqlx::sqlite::SqliteArguments<'q>) -> Result<(), BoxDynError> {
        sqlx::Arguments::add(arguments, *self)
    }
}

/// An owned value, kept apart from its statement (an entity's fields, a condition's values).
pub type Value = Box<dyn Param<'static>>;

/// A row a statement reads, through `sqlx::FromRow` on every driver: an entity, or a tuple such as `(i64,)`.
pub trait Record: RowOnPostgres + RowOnSqlite + Send + Unpin {}

impl<T: RowOnPostgres + RowOnSqlite + Send + Unpin> Record for T {}

/// Where statements run: a pool, or a transaction. `&Db` and `&mut Tx` turn into it.
pub enum On<'a> {
    Db(&'a Db),
    Tx(&'a mut Tx),
}

impl<'a> From<&'a Db> for On<'a> {
    fn from(db: &'a Db) -> Self {
        On::Db(db)
    }
}

impl<'a> From<&'a mut Tx> for On<'a> {
    fn from(tx: &'a mut Tx) -> Self {
        On::Tx(tx)
    }
}

impl<'a, 'b> From<&'a mut Scope<'b>> for On<'a> {
    fn from(scope: &'a mut Scope<'b>) -> Self {
        On::Tx(scope)
    }
}

impl<'a> On<'a> {
    /// A transaction of its own on a pool; the caller's inside a transaction.
    pub async fn transaction(self) -> Result<Scope<'a>, sqlx::Error> {
        match self {
            On::Db(db) => db.begin().await.map(Scope::Own),
            On::Tx(tx) => Ok(Scope::Within(tx)),
        }
    }
}

/// The text of a statement, standard and per driver where it differs, and the values bound to its placeholders.
struct Statement<'q> {
    standard: String,
    postgres: Option<String>,
    sqlite: Option<String>,
    params: Vec<Box<dyn Param<'q> + 'q>>,
}

/// A statement run for the rows it changes: `Db::query(sql)`, as `sqlx::query`.
pub struct Query<'q>(Statement<'q>);

/// A statement whose rows are read as `T`: `Db::query_as(sql)`, as `sqlx::query_as`.
pub struct QueryAs<'q, T>(Statement<'q>, PhantomData<fn() -> T>);

/// A statement whose first column is read as `T`: `Db::query_scalar(sql)`, as `sqlx::query_scalar`.
pub struct QueryScalar<'q, T>(Statement<'q>, PhantomData<fn() -> T>);

/// What a statement run by `execute` did.
#[derive(Debug)]
pub struct Done(u64);

impl Done {
    pub fn rows_affected(&self) -> u64 {
        self.0
    }
}

impl Db {
    /// A statement in standard SQL, run for the rows it changes.
    pub fn query<'q>(sql: impl Into<String>) -> Query<'q> {
        Query(Statement::new(sql.into()))
    }

    /// A statement in standard SQL whose rows are read as `T`.
    pub fn query_as<'q, T>(sql: impl Into<String>) -> QueryAs<'q, T> {
        QueryAs(Statement::new(sql.into()), PhantomData)
    }

    /// A statement in standard SQL whose first column is read as `T`.
    pub fn query_scalar<'q, T>(sql: impl Into<String>) -> QueryScalar<'q, T> {
        QueryScalar(Statement::new(sql.into()), PhantomData)
    }
}

impl Statement<'_> {
    fn new(standard: String) -> Self {
        Statement { standard, postgres: None, sqlite: None, params: vec![] }
    }
}

/// `.postgres(sql)`, `.sqlite(sql)` and `.bind(value)` of each kind of statement.
macro_rules! builder {
    ($statement:ident $(<$t:ident>)?, $($wrap:tt)*) => {
        impl<'q $(, $t)?> $statement<'q $(, $t)?> {
            /// The text on PostgreSQL, where its SQL differs from the standard.
            pub fn postgres(self, sql: impl Into<String>) -> Self {
                let statement = Statement { postgres: Some(sql.into()), ..self.0 };
                $($wrap)*(statement)
            }

            /// The text on SQLite, where its SQL differs from the standard.
            pub fn sqlite(self, sql: impl Into<String>) -> Self {
                let statement = Statement { sqlite: Some(sql.into()), ..self.0 };
                $($wrap)*(statement)
            }

            /// The value of the next placeholder.
            pub fn bind<V: Param<'q>>(self, value: V) -> Self {
                self.bind_all(vec![Box::new(value)])
            }

            /// The values of the next placeholders.
            pub fn bind_all(self, values: Vec<Box<dyn Param<'q> + 'q>>) -> Self {
                let mut statement = self.0;
                statement.params.extend(values);
                $($wrap)*(statement)
            }
        }
    };
}

builder!(Query, Query);
builder!(QueryAs<T>, (|statement| QueryAs(statement, PhantomData)));
builder!(QueryScalar<T>, (|statement| QueryScalar(statement, PhantomData)));

#[cfg(feature = "axum")]
impl Db {
    pub async fn get<T: Table>(&self, key: impl Into<Key>) -> ApiResult<T> {
        crud::get(self, &key.into()).await
    }

    pub async fn list<T: Table + serde::Serialize>(&self, request: &ListRequest) -> ApiResult<Page<T>> {
        crud::list(self, request).await
    }

    pub async fn select<T: Table>(&self, filter: Where) -> ApiResult<Vec<T>> {
        crud::select(self, filter).await
    }

    pub async fn create<T: crud::Row>(&self, data: T) -> ApiResult<T> {
        crud::create(self, data, None).await
    }

    pub async fn upsert<T: Table>(&self, data: T) -> ApiResult<T> {
        crud::upsert(self, data).await
    }

    pub async fn replace<T: crud::Row>(&self, data: T) -> ApiResult<T> {
        crud::replace(self, data, None).await
    }

    pub async fn patch<T: crud::Row>(&self, body: Changes) -> ApiResult<T> {
        crud::patch(self, body, None).await
    }

    pub async fn delete<T: crud::Row>(&self, key: impl Into<Key>) -> ApiResult<T> {
        crud::delete(self, &key.into(), None).await
    }
}

#[cfg(feature = "axum")]
impl Tx {
    pub async fn get<T: Table>(&mut self, key: impl Into<Key>) -> ApiResult<T> {
        crud::get(self, &key.into()).await
    }

    pub async fn select<T: Table>(&mut self, filter: Where) -> ApiResult<Vec<T>> {
        crud::select(self, filter).await
    }

    pub async fn create<T: crud::Row>(&mut self, data: T) -> ApiResult<T> {
        crud::create(self, data, None).await
    }

    pub async fn replace<T: crud::Row>(&mut self, data: T) -> ApiResult<T> {
        crud::replace(self, data, None).await
    }

    pub async fn patch<T: crud::Row>(&mut self, body: Changes) -> ApiResult<T> {
        crud::patch(self, body, None).await
    }

    pub async fn delete<T: crud::Row>(&mut self, key: impl Into<Key>) -> ApiResult<T> {
        crud::delete(self, &key.into(), None).await
    }
}

/// Runs `$statement` on `$on` with `sqlx::$make` and `.$method`, on the driver it is for; `$then` maps the driver's
/// result.
macro_rules! run {
    ($statement:expr, $on:expr, $make:ident, $method:ident, $then:expr) => {{
        #[allow(unused_variables)]
        let Statement { standard, postgres, sqlite, params } = $statement;
        match $on {
            #[cfg(feature = "postgres")]
            On::Db(Db::Postgres(pool)) => sqlx::$make(&numbered(&postgres.unwrap_or(standard)), postgres_arguments(params)?).$method(pool).await.map($then),
            #[cfg(feature = "postgres")]
            On::Tx(Tx::Postgres(tx)) => sqlx::$make(&numbered(&postgres.unwrap_or(standard)), postgres_arguments(params)?).$method(&mut **tx).await.map($then),
            #[cfg(feature = "sqlite")]
            On::Db(Db::Sqlite(pool)) => sqlx::$make(&sqlite.unwrap_or(standard), sqlite_arguments(params)?).$method(pool).await.map($then),
            #[cfg(feature = "sqlite")]
            On::Tx(Tx::Sqlite(tx)) => sqlx::$make(&sqlite.unwrap_or(standard), sqlite_arguments(params)?).$method(&mut **tx).await.map($then),
        }
    }};
}

impl Query<'_> {
    pub async fn execute<'a>(self, on: impl Into<On<'a>>) -> Result<Done, sqlx::Error> {
        run!(self.0, on.into(), query_with, execute, |done| Done(done.rows_affected()))
    }
}

impl<T: Record> QueryAs<'_, T> {
    pub async fn fetch_all<'a>(self, on: impl Into<On<'a>>) -> Result<Vec<T>, sqlx::Error> {
        run!(self.0, on.into(), query_as_with, fetch_all, std::convert::identity)
    }

    pub async fn fetch_one<'a>(self, on: impl Into<On<'a>>) -> Result<T, sqlx::Error> {
        run!(self.0, on.into(), query_as_with, fetch_one, std::convert::identity)
    }

    pub async fn fetch_optional<'a>(self, on: impl Into<On<'a>>) -> Result<Option<T>, sqlx::Error> {
        run!(self.0, on.into(), query_as_with, fetch_optional, std::convert::identity)
    }
}

impl<T> QueryScalar<'_, T>
where
    (T,): Record,
{
    pub async fn fetch_all<'a>(self, on: impl Into<On<'a>>) -> Result<Vec<T>, sqlx::Error> {
        let rows: Vec<(T,)> = QueryAs::<(T,)>(self.0, PhantomData).fetch_all(on).await?;
        Ok(rows.into_iter().map(|(value,)| value).collect())
    }

    pub async fn fetch_one<'a>(self, on: impl Into<On<'a>>) -> Result<T, sqlx::Error> {
        QueryAs::<(T,)>(self.0, PhantomData).fetch_one(on).await.map(|(value,)| value)
    }

    pub async fn fetch_optional<'a>(self, on: impl Into<On<'a>>) -> Result<Option<T>, sqlx::Error> {
        QueryAs::<(T,)>(self.0, PhantomData).fetch_optional(on).await.map(|row| row.map(|(value,)| value))
    }
}

/// `?` placeholders as PostgreSQL numbers them (`$1`, `$2`, …), leaving quoted text alone.
#[cfg(feature = "postgres")]
fn numbered(text: &str) -> String {
    text.chars()
        .scan((false, 0), |(quoted, count), char| {
            Some(match (char, *quoted) {
                ('\'', _) => {
                    *quoted = !*quoted;
                    char.to_string()
                }
                ('?', false) => {
                    *count += 1;
                    format!("${count}")
                }
                _ => char.to_string(),
            })
        })
        .collect()
}

#[cfg(feature = "postgres")]
fn postgres_arguments<'q>(params: Vec<Box<dyn Param<'q> + 'q>>) -> Result<sqlx::postgres::PgArguments, sqlx::Error> {
    params.into_iter().try_fold(Default::default(), |mut arguments, param| {
        param.add_postgres(&mut arguments).map_err(sqlx::Error::Encode)?;
        Ok(arguments)
    })
}

#[cfg(feature = "sqlite")]
fn sqlite_arguments<'q>(params: Vec<Box<dyn Param<'q> + 'q>>) -> Result<sqlx::sqlite::SqliteArguments<'q>, sqlx::Error> {
    params.into_iter().try_fold(Default::default(), |mut arguments, param| {
        param.add_sqlite(&mut arguments).map_err(sqlx::Error::Encode)?;
        Ok(arguments)
    })
}
