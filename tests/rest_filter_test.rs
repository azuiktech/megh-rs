//! B2: an AIP-160 filter becomes a SQL WHERE condition.

use megh::auth::filter::push_filter;
use megh::ColumnMeta;
use sqlx::{PgPool, QueryBuilder};

const fn column(name: &'static str) -> ColumnMeta {
    ColumnMeta { name, is_primary_key: false, is_json: false, is_array: false, has_default: false }
}
const COLUMNS: &[ColumnMeta] = &[column("id"), column("name"), column("capacity"), column("opens_at"), ColumnMeta { is_array: true, ..column("staff_ids") }];

async fn names(pool: &PgPool, filter: &str) -> Result<Vec<String>, String> {
    let mut query = QueryBuilder::new("SELECT name FROM rooms WHERE true");
    if !filter.is_empty() {
        let filter = aip_160::parse_filter(filter).map_err(|e| e.to_string())?;
        push_filter(&mut query, COLUMNS, &filter).map_err(|e| e.to_string())?;
    }
    query.push(" ORDER BY name");
    query.build_query_scalar().fetch_all(pool).await.map_err(|e| e.to_string())
}

#[sqlx::test(migrations = false)]
async fn filter_becomes_a_where_condition(pool: PgPool) {
    sqlx::raw_sql(
        "CREATE TABLE rooms (id UUID, name TEXT, capacity INT, opens_at TIMESTAMPTZ, staff_ids INT[], attributes JSONB NOT NULL);
         INSERT INTO rooms VALUES
           ('9d4da323-4c20-360f-bd9b-ec54feec54f0', 'Lobby', 10, '2026-10-20 08:00+00', '{42}', '{\"sea_facing\": true, \"floor\": 0}'),
           ('0192f0c4-0000-7000-8000-000000000001', 'Room 1', 2, '2026-10-20 09:00+00', '{7}', '{\"sea_facing\": false, \"floor\": 1, \"tags\": [\"quiet\"]}'),
           ('0192f0c4-0000-7000-8000-000000000002', 'O''Brien room', 1, '2026-10-21 09:00+00', '{}', '{\"floor\": \"9\"}');",
    )
    .execute(&pool)
    .await
    .unwrap();

    assert_eq!(names(&pool, "").await.unwrap(), ["Lobby", "O'Brien room", "Room 1"]);
    assert_eq!(names(&pool, "capacity > 5").await.unwrap(), ["Lobby"]);
    assert_eq!(names(&pool, "id = \"9d4da323-4c20-360f-bd9b-ec54feec54f0\"").await.unwrap(), ["Lobby"]);
    assert_eq!(names(&pool, "opens_at >= \"2026-10-20T09:00:00Z\"").await.unwrap(), ["O'Brien room", "Room 1"]);
    assert_eq!(names(&pool, "staff_ids:42").await.unwrap(), ["Lobby"]);
    assert!(names(&pool, "name = \"O'Brien room\"").await.is_err(), "aip-160 0.1.5 allows no quote inside a value");
    assert_eq!(names(&pool, "capacity < 5 AND NOT name = \"Room 1\"").await.unwrap(), ["O'Brien room"]);
    assert_eq!(names(&pool, "sea_facing = true OR capacity = 1").await.unwrap(), ["Lobby", "O'Brien room"]);
    assert_eq!(names(&pool, "floor > 0").await.unwrap(), ["Room 1"], "the room whose floor is the text \"9\" is not a number");
    assert_eq!(names(&pool, "tags:\"quiet\"").await.unwrap(), ["Room 1"]);
    assert!(names(&pool, "capacity > null").await.is_err());
    assert!(names(&pool, "name = \"x'; DROP TABLE rooms; --\"").await.is_err());
    assert_eq!(names(&pool, "").await.unwrap().len(), 3, "table still there");
}
