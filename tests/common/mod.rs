//! Runs a test on SQLite and on PostgreSQL, each with megh's tables: `dual!(name)` above `async fn name(pool: Db)`.

use megh::Db;

/// A new uuid v7 in SQLite SQL, where a test's PostgreSQL SQL calls `uuidv7()`.
#[allow(dead_code)]
pub const SQLITE_UUIDV7: &str = "(unhex(printf('%012x', CAST(unixepoch('subsec') * 1000 AS INTEGER)) || '7' || substr(hex(randomblob(2)), 2, 3) || substr('89ab', 1 + abs(random()) % 4, 1) || substr(hex(randomblob(2)), 2, 3) || hex(randomblob(6))))";

/// A new SQLite database in a file of its own, so concurrent requests wait for each other as they would in production.
pub async fn sqlite() -> Db {
    let path = std::env::temp_dir().join(format!("megh-{}.db", uuid::Uuid::now_v7()));
    let options = sqlx::sqlite::SqliteConnectOptions::new().filename(path).create_if_missing(true).foreign_keys(true);
    let db = Db::Sqlite(sqlx::SqlitePool::connect_with(options).await.unwrap());
    db.migrate().await.unwrap();
    db
}

/// The test database sqlx made, with megh's tables.
pub async fn postgres(pool: sqlx::PgPool) -> Db {
    let db = Db::Postgres(pool);
    db.migrate().await.unwrap();
    db
}

/// Runs the test `$name` once on SQLite and once on PostgreSQL.
#[macro_export]
macro_rules! dual {
    ($name:ident) => {
        mod $name {
            #[tokio::test]
            async fn sqlite() {
                super::$name($crate::common::sqlite().await).await
            }

            #[sqlx::test(migrations = false)]
            async fn postgres(pool: sqlx::PgPool) {
                super::$name($crate::common::postgres(pool).await).await
            }
        }
    };
}

/// Runs the test `$name`, which reads PostgreSQL's own catalogs, on PostgreSQL only.
#[macro_export]
macro_rules! postgres {
    ($name:ident) => {
        mod $name {
            #[sqlx::test(migrations = false)]
            async fn postgres(pool: sqlx::PgPool) {
                super::$name($crate::common::postgres(pool).await).await
            }
        }
    };
}

/// Runs `sql`, one or more statements, as it is.
#[allow(dead_code)]
pub async fn raw(db: &Db, sql: &str) -> Result<(), sqlx::Error> {
    match db {
        Db::Postgres(pool) => sqlx::raw_sql(sql).execute(pool).await.map(drop),
        Db::Sqlite(pool) => sqlx::raw_sql(sql).execute(pool).await.map(drop),
    }
}
