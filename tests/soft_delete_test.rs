//! Soft delete: `crud::delete` marks `deleted_at` and follows `ON DELETE CASCADE` foreign keys (PostgreSQL).

#[macro_use]
mod common;

use chrono::{DateTime, Utc};
use megh::auth::rest::{ApiError, Key};
use megh::auth::crud;
use megh::entity;
use serde::{Deserialize, Serialize};
use megh::Db;
use uuid::Uuid;
use validator::Validate;

#[entity(table = "parents", keys = ["id"], timestamps, soft_delete)]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Validate)]
struct Parent {
    id: Uuid,
    name: String,
}

#[entity(table = "nodes", keys = ["id"], timestamps, soft_delete)]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Validate)]
struct Node {
    id: Uuid,
    parent_id: Option<Uuid>,
    name: String,
}

const STAMPS: &str = "created_at TIMESTAMPTZ NOT NULL DEFAULT now(), updated_at TIMESTAMPTZ NOT NULL DEFAULT now(), deleted_at TIMESTAMPTZ";

async fn schema(pool: &Db) {
    let tables = [
        format!("CREATE TABLE parents (id UUID PRIMARY KEY, name TEXT NOT NULL, {STAMPS})"),
        format!("CREATE TABLE children (id UUID PRIMARY KEY, parent_id UUID NOT NULL REFERENCES parents(id) ON DELETE CASCADE, {STAMPS})"),
        format!("CREATE TABLE grandchildren (id UUID PRIMARY KEY, child_id UUID NOT NULL REFERENCES children(id) ON DELETE CASCADE, {STAMPS})"),
        format!("CREATE TABLE notes (id UUID PRIMARY KEY, parent_id UUID NOT NULL REFERENCES parents(id), {STAMPS})"),
        format!("CREATE TABLE nodes (id UUID PRIMARY KEY, parent_id UUID REFERENCES nodes(id) ON DELETE CASCADE, name TEXT NOT NULL, {STAMPS})"),
    ];
    for sql in tables {
        Db::query(&sql).sqlite(sql.replace("now()", "(strftime('%Y-%m-%dT%H:%M:%f+00:00'))")).execute(pool).await.unwrap();
    }
}

fn key(id: Uuid) -> Key {
    Key { tenant_id: Uuid::nil(), id }
}

async fn parent(pool: &Db, name: &str) -> Parent {
    let data = serde_json::from_value(serde_json::json!({"id": Uuid::nil(), "name": name})).unwrap();
    crud::create(pool, data, None).await.unwrap()
}

async fn node(pool: &Db, parent_id: Option<Uuid>, name: &str) -> Node {
    let data = serde_json::from_value(serde_json::json!({"id": Uuid::nil(), "parent_id": parent_id, "name": name})).unwrap();
    crud::create(pool, data, None).await.unwrap()
}

async fn row(pool: &Db, table: &str, parent_column: &str, parent: Uuid) -> Uuid {
    let id = Uuid::now_v7();
    Db::query(&format!("INSERT INTO {table} (id, {parent_column}) VALUES ($1, $2)")).bind(id).bind(parent).execute(pool).await.unwrap();
    id
}

async fn deleted_at(pool: &Db, table: &str, id: Uuid) -> Option<DateTime<Utc>> {
    Db::query_scalar(&format!("SELECT deleted_at FROM {table} WHERE id = $1")).bind(id).fetch_one(pool).await.unwrap()
}

dual!(a_new_row_is_not_deleted);
async fn a_new_row_is_not_deleted(pool: Db) {
    schema(&pool).await;
    let p = parent(&pool, "p").await;
    assert_eq!(p.deleted_at, None);
    assert!(serde_json::to_value(&p).unwrap()["deleted_at"].is_null());
}

dual!(soft_delete_marks_the_row_and_keeps_it);
async fn soft_delete_marks_the_row_and_keeps_it(pool: Db) {
    schema(&pool).await;
    let p = parent(&pool, "p").await;
    let deleted = crud::delete::<Parent>(&pool, &key(p.id), None).await.unwrap();
    assert!(deleted.deleted_at.is_some());
    assert!(deleted.updated_at >= p.updated_at);
    let count: i64 = Db::query_scalar("SELECT count(*) FROM parents WHERE id = $1").bind(p.id).fetch_one(&pool).await.unwrap();
    assert_eq!(count, 1, "the row stays");
    assert_eq!(deleted_at(&pool, "parents", p.id).await, deleted.deleted_at);
}

dual!(deleting_twice_or_deleting_a_missing_row_is_not_found_and_keeps_the_first_timestamp);
async fn deleting_twice_or_deleting_a_missing_row_is_not_found_and_keeps_the_first_timestamp(pool: Db) {
    schema(&pool).await;
    let p = parent(&pool, "p").await;
    let first = crud::delete::<Parent>(&pool, &key(p.id), None).await.unwrap().deleted_at;
    assert!(matches!(crud::delete::<Parent>(&pool, &key(p.id), None).await, Err(ApiError::NotFound)));
    assert_eq!(deleted_at(&pool, "parents", p.id).await, first);
    assert!(matches!(crud::delete::<Parent>(&pool, &key(Uuid::now_v7()), None).await, Err(ApiError::NotFound)));
}

dual!(an_update_of_a_deleted_row_is_refused);
async fn an_update_of_a_deleted_row_is_refused(pool: Db) {
    schema(&pool).await;
    let p = parent(&pool, "p").await;
    crud::delete::<Parent>(&pool, &key(p.id), None).await.unwrap();
    let renamed = Parent { name: "renamed".into(), ..p.clone() };
    assert!(matches!(crud::replace(&pool, renamed, None).await, Err(ApiError::NotFound)));
    let name: String = Db::query_scalar("SELECT name FROM parents WHERE id = $1").bind(p.id).fetch_one(&pool).await.unwrap();
    assert_eq!(name, "p");
}

dual!(an_upsert_does_not_revive_a_deleted_row);
async fn an_upsert_does_not_revive_a_deleted_row(pool: Db) {
    schema(&pool).await;
    let p = parent(&pool, "p").await;
    crud::delete::<Parent>(&pool, &key(p.id), None).await.unwrap();
    let _ = crud::upsert(&pool, p.clone()).await;
    assert!(deleted_at(&pool, "parents", p.id).await.is_some());
}

dual!(a_delete_reaches_every_level_below_through_cascading_foreign_keys_with_one_timestamp);
async fn a_delete_reaches_every_level_below_through_cascading_foreign_keys_with_one_timestamp(pool: Db) {
    schema(&pool).await;
    let (doomed, safe) = (parent(&pool, "doomed").await, parent(&pool, "safe").await);
    let child = row(&pool, "children", "parent_id", doomed.id).await;
    let grandchild = row(&pool, "grandchildren", "child_id", child).await;
    let other_child = row(&pool, "children", "parent_id", safe.id).await;

    let stamp = crud::delete::<Parent>(&pool, &key(doomed.id), None).await.unwrap().deleted_at;
    assert!(stamp.is_some());
    assert_eq!(deleted_at(&pool, "children", child).await, stamp);
    assert_eq!(deleted_at(&pool, "grandchildren", grandchild).await, stamp);
    assert_eq!(deleted_at(&pool, "parents", safe.id).await, None);
    assert_eq!(deleted_at(&pool, "children", other_child).await, None);
}

dual!(a_dependant_that_was_already_deleted_keeps_its_own_timestamp);
async fn a_dependant_that_was_already_deleted_keeps_its_own_timestamp(pool: Db) {
    schema(&pool).await;
    let p = parent(&pool, "p").await;
    let earlier = row(&pool, "children", "parent_id", p.id).await;
    Db::query("UPDATE children SET deleted_at = '2020-01-01T00:00:00Z' WHERE id = $1").bind(earlier).execute(&pool).await.unwrap();
    crud::delete::<Parent>(&pool, &key(p.id), None).await.unwrap();
    assert_eq!(deleted_at(&pool, "children", earlier).await.unwrap().to_rfc3339(), "2020-01-01T00:00:00+00:00");
}

dual!(a_foreign_key_without_on_delete_cascade_is_not_followed);
async fn a_foreign_key_without_on_delete_cascade_is_not_followed(pool: Db) {
    schema(&pool).await;
    let p = parent(&pool, "p").await;
    let note = row(&pool, "notes", "parent_id", p.id).await;
    crud::delete::<Parent>(&pool, &key(p.id), None).await.unwrap();
    assert_eq!(deleted_at(&pool, "notes", note).await, None);
}

dual!(a_tree_that_references_itself_is_deleted_down_to_the_leaves);
async fn a_tree_that_references_itself_is_deleted_down_to_the_leaves(pool: Db) {
    schema(&pool).await;
    let root = node(&pool, None, "root").await;
    let branch = node(&pool, Some(root.id), "branch").await;
    let leaf = node(&pool, Some(branch.id), "leaf").await;
    let sibling = node(&pool, None, "sibling").await;
    let sibling_child = node(&pool, Some(sibling.id), "sibling child").await;

    let stamp = crud::delete::<Node>(&pool, &key(root.id), None).await.unwrap().deleted_at;
    assert_eq!(deleted_at(&pool, "nodes", branch.id).await, stamp);
    assert_eq!(deleted_at(&pool, "nodes", leaf.id).await, stamp);
    assert_eq!(deleted_at(&pool, "nodes", sibling.id).await, None);
    assert_eq!(deleted_at(&pool, "nodes", sibling_child.id).await, None);
}

dual!(a_dependant_table_without_deleted_at_fails_the_delete_and_changes_nothing);
async fn a_dependant_table_without_deleted_at_fails_the_delete_and_changes_nothing(pool: Db) {
    schema(&pool).await;
    Db::query("CREATE TABLE bare_children (id UUID PRIMARY KEY, parent_id UUID NOT NULL REFERENCES parents(id) ON DELETE CASCADE)")
        .execute(&pool).await.unwrap();
    let p = parent(&pool, "p").await;
    let child = row(&pool, "children", "parent_id", p.id).await;
    row(&pool, "bare_children", "parent_id", p.id).await;

    assert!(crud::delete::<Parent>(&pool, &key(p.id), None).await.is_err());
    assert_eq!(deleted_at(&pool, "parents", p.id).await, None, "the parent is not marked");
    assert_eq!(deleted_at(&pool, "children", child).await, None, "nothing else is marked");
}
