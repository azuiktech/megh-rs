use sqlx::sqlite::SqlitePoolOptions;
use sqlx::{SqlitePool, Type};
use uuid::Uuid;

#[derive(Debug, PartialEq)]
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

#[derive(Debug, PartialEq)]
pub struct Celsius(f64);

megh::sqlx_type!(Celsius, match driver {
    Postgres => (
        |value: &Celsius| -> f64 { value.0 },
        |stored: f64| Ok(Celsius(stored)),
    ),
    Sqlite => (
        |value: &Celsius| -> i64 { (value.0 * 100.0) as i64 },
        |stored: i64| Ok(Celsius(stored as f64 / 100.0)),
    ),
});

async fn sqlite() -> SqlitePool {
    let pool = SqlitePoolOptions::new().max_connections(1).connect("sqlite::memory:").await.unwrap();
    sqlx::raw_sql("CREATE TABLE things (id TEXT, heat INTEGER)").execute(&pool).await.unwrap();
    pool
}

#[tokio::test]
async fn fallback_arm_types_the_value_as_its_stored_type() {
    assert_eq!(<MyUuid as Type<sqlx::Sqlite>>::type_info(), <String as Type<sqlx::Sqlite>>::type_info());
}

#[tokio::test]
async fn fallback_arm_writes_through_the_write_closure() {
    let pool = sqlite().await;
    let id = Uuid::now_v7();
    sqlx::query("INSERT INTO things (id) VALUES (?)").bind(MyUuid(id)).execute(&pool).await.unwrap();
    let stored: String = sqlx::query_scalar("SELECT id FROM things").fetch_one(&pool).await.unwrap();
    assert_eq!(stored, id.to_string());
}

#[tokio::test]
async fn fallback_arm_reads_through_the_read_closure() {
    let pool = sqlite().await;
    let id = Uuid::now_v7();
    sqlx::query("INSERT INTO things (id) VALUES (?)").bind(id.to_string()).execute(&pool).await.unwrap();
    let read: MyUuid = sqlx::query_scalar("SELECT id FROM things").fetch_one(&pool).await.unwrap();
    assert_eq!(read, MyUuid(id));
}

#[tokio::test]
async fn read_closure_error_fails_the_decode() {
    let pool = sqlite().await;
    sqlx::query("INSERT INTO things (id) VALUES ('not a uuid')").execute(&pool).await.unwrap();
    let read = sqlx::query_scalar::<_, MyUuid>("SELECT id FROM things").fetch_one(&pool).await;
    assert!(matches!(read, Err(sqlx::Error::ColumnDecode { .. })));
}

#[tokio::test]
async fn named_arm_is_used_for_its_driver() {
    let pool = sqlite().await;
    sqlx::query("INSERT INTO things (heat) VALUES (?)").bind(Celsius(21.5)).execute(&pool).await.unwrap();
    let stored: i64 = sqlx::query_scalar("SELECT heat FROM things").fetch_one(&pool).await.unwrap();
    let read: Celsius = sqlx::query_scalar("SELECT heat FROM things").fetch_one(&pool).await.unwrap();
    assert_eq!((stored, read), (2150, Celsius(21.5)));
}

#[cfg(feature = "postgres")]
#[sqlx::test(migrations = false)]
async fn postgres_arm_stores_its_own_type(pool: sqlx::PgPool) {
    sqlx::raw_sql("CREATE TABLE things (id uuid, heat double precision)").execute(&pool).await.unwrap();
    let id = Uuid::now_v7();
    sqlx::query("INSERT INTO things (id, heat) VALUES ($1, $2)").bind(MyUuid(id)).bind(Celsius(21.5)).execute(&pool).await.unwrap();
    let stored: (Uuid, f64) = sqlx::query_as("SELECT id, heat FROM things").fetch_one(&pool).await.unwrap();
    let read: (MyUuid, Celsius) = sqlx::query_as("SELECT id, heat FROM things").fetch_one(&pool).await.unwrap();
    assert_eq!((stored, read), ((id, 21.5), (MyUuid(id), Celsius(21.5))));
}
