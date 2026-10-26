//! Issue #222 (#20 of the code review): on SQLite a list column is JSON text, and `column:value` finds the rows whose
//! list holds the value.

use aip_160::Value;
use megh::auth::filter::Has;
use megh::ColumnMeta;
use sqlx::{Sqlite, SqlitePool};

const fn column(name: &'static str, is_json: bool, is_array: bool) -> ColumnMeta {
    ColumnMeta { name, is_primary_key: false, is_json, is_array, has_default: false }
}

async fn ids_where(pool: &SqlitePool, column: &ColumnMeta, value: Value) -> Vec<i64> {
    let condition = Sqlite::has(column, &value).unwrap();
    sqlx::query_scalar(&format!("SELECT id FROM schedules WHERE {condition} ORDER BY id")).fetch_all(pool).await.unwrap()
}

#[tokio::test]
async fn a_list_column_holds_the_value() {
    let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
    sqlx::raw_sql(
        r#"CREATE TABLE schedules (id INTEGER, name TEXT, service_ids TEXT, tags TEXT);
           INSERT INTO schedules VALUES
             (1, 'Cleaning', '["a1", "b2"]', '["quiet"]'),
             (2, 'Whitening', '["b2"]', '[7]'),
             (3, 'Closed', '[]', '{"quiet": true}');"#,
    )
    .execute(&pool)
    .await
    .unwrap();

    assert_eq!(ids_where(&pool, &column("service_ids", false, true), Value::String("b2".into())).await, [1, 2], "an array column");
    assert_eq!(ids_where(&pool, &column("tags", true, false), Value::String("quiet".into())).await, [1], "a JSON array column");
    assert_eq!(ids_where(&pool, &column("tags", true, false), Value::Number(7.0)).await, [2], "a number in a JSON array");
    assert!(Sqlite::has(&column("name", false, false), &Value::String("Cleaning".into())).is_err(), "name is not a list");
}
