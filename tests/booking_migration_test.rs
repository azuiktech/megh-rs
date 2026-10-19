//! Specification: migration 0020 rewrites what was stored with the old tags: a duration as ISO 8601 text instead of
//! `{"secs", "nanos"}`, a change and a settlement tagged `{"type", "value"}`; running the migrations again changes nothing.

mod booking_support;

use booking_support::*;
use serde_json::{json, Value};
use sqlx::PgPool;
use uuid::Uuid;

async fn stored(pool: &PgPool, column: &str, table: &str, id: Uuid) -> Value {
    let found: sqlx::types::Json<Value> = sqlx::query_scalar(&format!("SELECT {column} FROM {table} WHERE id = $1")).bind(id).fetch_one(pool).await.unwrap();
    found.0
}

#[sqlx::test]
async fn old_durations_and_tags_are_rewritten_once(pool: PgPool) {
    let org = org(&pool, "old").await;
    let (service, schedule, method) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
    let steps = json!([{"duration": {"secs": 3600, "nanos": 0}, "client": "present", "needs": [], "attributes": {}}]);
    let variants = json!([{"when": {"Filter": ""}, "duration": {"add": {"secs": 900, "nanos": 500000000}}, "price_micros": {"set": 5}}]);
    sqlx::query("INSERT INTO services (id, org_id, name, mode, price, steps, variants, attributes, created_at, updated_at) VALUES ($1, $2, 's', 'appointment', '{\"micros\": 1, \"currency\": \"USD\"}', $3, $4, '{}', now(), now())")
        .bind(service).bind(org).bind(&steps).bind(&variants).execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO schedules (id, org_id, kind, recurrence, span, created_at, updated_at) VALUES ($1, $2, 'open', 'DTSTART:20261104T090000Z\nRRULE:FREQ=DAILY', '{\"secs\": 28800, \"nanos\": 0}', now(), now())").bind(schedule).bind(org).execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO payment_methods (id, org_id, name, eligible, settlement, attributes, created_at, updated_at) VALUES ($1, $2, 'm', '{\"Filter\": \"\"}', '{\"prepaid\": {\"balance\": {\"uses\": 3}, \"valid\": null}}', '{}', now(), now())").bind(method).bind(org).execute(&pool).await.unwrap();
    for _ in 0..2 {
        megh::migrate(&pool).await.unwrap();
        assert_eq!(stored(&pool, "steps", "services", service).await[0]["duration"], json!("PT3600S"));
        assert_eq!(stored(&pool, "variants", "services", service).await[0], json!({"when": {"Filter": ""}, "duration": {"type": "add", "value": "PT900.5S"}, "price_micros": {"type": "set", "value": 5}}));
        assert_eq!(stored(&pool, "span", "schedules", schedule).await, json!("PT28800S"));
        assert_eq!(stored(&pool, "settlement", "payment_methods", method).await, json!({"type": "prepaid", "value": {"balance": {"uses": 3}, "valid": null}}));
    }
}
