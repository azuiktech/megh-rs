//! Soft delete: `Entity.deleted_at`, `TableEntity::soft_delete` and its cascade along `ON DELETE CASCADE` foreign keys.

use chrono::{DateTime, Utc};
use megh::{ColumnMeta, Entity, Table, TableEntity, NOT_DELETED};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, sqlx::FromRow, Table)]
#[table(name = "parents", keys = ["id"])]
struct ParentData {
    name: String,
}

type Parent = Entity<Uuid, ParentData>;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, sqlx::FromRow, Table)]
#[table(name = "nodes", keys = ["id"])]
struct NodeData {
    parent_id: Option<Uuid>,
    name: String,
}

type Node = Entity<Uuid, NodeData>;

const STAMPS: &str = "created_at TIMESTAMPTZ NOT NULL DEFAULT now(), updated_at TIMESTAMPTZ NOT NULL DEFAULT now(), deleted_at TIMESTAMPTZ";

async fn schema(pool: &PgPool) {
    let tables = [
        format!("CREATE TABLE parents (id UUID PRIMARY KEY, name TEXT NOT NULL, {STAMPS})"),
        format!("CREATE TABLE children (id UUID PRIMARY KEY, parent_id UUID NOT NULL REFERENCES parents(id) ON DELETE CASCADE, {STAMPS})"),
        format!("CREATE TABLE grandchildren (id UUID PRIMARY KEY, child_id UUID NOT NULL REFERENCES children(id) ON DELETE CASCADE, {STAMPS})"),
        format!("CREATE TABLE notes (id UUID PRIMARY KEY, parent_id UUID NOT NULL REFERENCES parents(id), {STAMPS})"),
        format!("CREATE TABLE nodes (id UUID PRIMARY KEY, parent_id UUID REFERENCES nodes(id) ON DELETE CASCADE, name TEXT NOT NULL, {STAMPS})"),
    ];
    for sql in tables {
        sqlx::query(&sql).execute(pool).await.unwrap();
    }
}

async fn parent(pool: &PgPool, name: &str) -> Parent {
    Parent::new(Uuid::new_v4(), ParentData { name: name.into() }).insert(pool).await.unwrap()
}

async fn row(pool: &PgPool, table: &str, parent_column: &str, parent: Uuid) -> Uuid {
    let id = Uuid::new_v4();
    sqlx::query(&format!("INSERT INTO {table} (id, {parent_column}) VALUES ($1, $2)")).bind(id).bind(parent).execute(pool).await.unwrap();
    id
}

async fn deleted_at(pool: &PgPool, table: &str, id: Uuid) -> Option<DateTime<Utc>> {
    sqlx::query_scalar(&format!("SELECT deleted_at FROM {table} WHERE id = $1")).bind(id).fetch_one(pool).await.unwrap()
}

#[test]
fn a_new_entity_is_not_deleted_and_shows_deleted_at_as_null() {
    let entity = Parent::new(Uuid::new_v4(), ParentData { name: "p".into() });
    assert_eq!(entity.deleted_at, None);
    assert!(serde_json::to_value(&entity).unwrap()["deleted_at"].is_null());
}

#[test]
fn reads_add_one_fixed_condition() {
    assert_eq!(NOT_DELETED, "deleted_at IS NULL");
}

#[sqlx::test(migrations = false)]
async fn soft_delete_marks_the_row_and_keeps_it(pool: PgPool) {
    schema(&pool).await;
    let p = parent(&pool, "p").await;
    let deleted = p.soft_delete(&pool).await.unwrap().expect("the row is deleted");
    assert!(deleted.deleted_at.is_some());
    assert!(deleted.updated_at >= p.updated_at);
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM parents WHERE id = $1").bind(p.id).fetch_one(&pool).await.unwrap();
    assert_eq!(count, 1, "the row stays");
    assert_eq!(deleted_at(&pool, "parents", p.id).await, deleted.deleted_at);
}

#[sqlx::test(migrations = false)]
async fn deleting_twice_or_deleting_a_missing_row_returns_nothing_and_keeps_the_first_timestamp(pool: PgPool) {
    schema(&pool).await;
    let p = parent(&pool, "p").await;
    let first = p.soft_delete(&pool).await.unwrap().expect("first delete").deleted_at;
    assert!(p.soft_delete(&pool).await.unwrap().is_none());
    assert_eq!(deleted_at(&pool, "parents", p.id).await, first);
    let missing = Parent::new(Uuid::new_v4(), ParentData { name: "ghost".into() });
    assert!(missing.soft_delete(&pool).await.unwrap().is_none());
}

#[sqlx::test(migrations = false)]
async fn an_update_of_a_deleted_row_is_refused(pool: PgPool) {
    schema(&pool).await;
    let p = parent(&pool, "p").await;
    p.soft_delete(&pool).await.unwrap();
    let mut renamed = p.clone();
    renamed.name = "renamed".into();
    assert!(matches!(renamed.update(&pool).await, Err(sqlx::Error::RowNotFound)));
    let name: String = sqlx::query_scalar("SELECT name FROM parents WHERE id = $1").bind(p.id).fetch_one(&pool).await.unwrap();
    assert_eq!(name, "p");
}

#[sqlx::test(migrations = false)]
async fn an_upsert_does_not_revive_a_deleted_row(pool: PgPool) {
    schema(&pool).await;
    let p = parent(&pool, "p").await;
    p.soft_delete(&pool).await.unwrap();
    let _ = p.upsert(&pool).await;
    assert!(deleted_at(&pool, "parents", p.id).await.is_some());
}

#[sqlx::test(migrations = false)]
async fn a_delete_reaches_every_level_below_through_cascading_foreign_keys_with_one_timestamp(pool: PgPool) {
    schema(&pool).await;
    let (doomed, safe) = (parent(&pool, "doomed").await, parent(&pool, "safe").await);
    let child = row(&pool, "children", "parent_id", doomed.id).await;
    let grandchild = row(&pool, "grandchildren", "child_id", child).await;
    let other_child = row(&pool, "children", "parent_id", safe.id).await;

    let stamp = doomed.soft_delete(&pool).await.unwrap().expect("deleted").deleted_at;
    assert!(stamp.is_some());
    assert_eq!(deleted_at(&pool, "children", child).await, stamp);
    assert_eq!(deleted_at(&pool, "grandchildren", grandchild).await, stamp);
    assert_eq!(deleted_at(&pool, "parents", safe.id).await, None);
    assert_eq!(deleted_at(&pool, "children", other_child).await, None);
}

#[sqlx::test(migrations = false)]
async fn a_dependant_that_was_already_deleted_keeps_its_own_timestamp(pool: PgPool) {
    schema(&pool).await;
    let p = parent(&pool, "p").await;
    let earlier = row(&pool, "children", "parent_id", p.id).await;
    sqlx::query("UPDATE children SET deleted_at = '2020-01-01T00:00:00Z' WHERE id = $1").bind(earlier).execute(&pool).await.unwrap();
    p.soft_delete(&pool).await.unwrap();
    assert_eq!(deleted_at(&pool, "children", earlier).await.unwrap().to_rfc3339(), "2020-01-01T00:00:00+00:00");
}

#[sqlx::test(migrations = false)]
async fn a_foreign_key_without_on_delete_cascade_is_not_followed(pool: PgPool) {
    schema(&pool).await;
    let p = parent(&pool, "p").await;
    let note = row(&pool, "notes", "parent_id", p.id).await;
    p.soft_delete(&pool).await.unwrap();
    assert_eq!(deleted_at(&pool, "notes", note).await, None);
}

#[sqlx::test(migrations = false)]
async fn a_tree_that_references_itself_is_deleted_down_to_the_leaves(pool: PgPool) {
    schema(&pool).await;
    let node = |parent_id: Option<Uuid>, name: &str| Node::new(Uuid::new_v4(), NodeData { parent_id, name: name.into() });
    let root = node(None, "root").insert(&pool).await.unwrap();
    let branch = node(Some(root.id), "branch").insert(&pool).await.unwrap();
    let leaf = node(Some(branch.id), "leaf").insert(&pool).await.unwrap();
    let sibling = node(None, "sibling").insert(&pool).await.unwrap();
    let sibling_child = node(Some(sibling.id), "sibling child").insert(&pool).await.unwrap();

    let stamp = root.soft_delete(&pool).await.unwrap().expect("deleted").deleted_at;
    assert_eq!(deleted_at(&pool, "nodes", branch.id).await, stamp);
    assert_eq!(deleted_at(&pool, "nodes", leaf.id).await, stamp);
    assert_eq!(deleted_at(&pool, "nodes", sibling.id).await, None);
    assert_eq!(deleted_at(&pool, "nodes", sibling_child.id).await, None);
}

#[sqlx::test(migrations = false)]
async fn a_dependant_table_without_deleted_at_fails_the_delete_and_changes_nothing(pool: PgPool) {
    schema(&pool).await;
    sqlx::query("CREATE TABLE bare_children (id UUID PRIMARY KEY, parent_id UUID NOT NULL REFERENCES parents(id) ON DELETE CASCADE)")
        .execute(&pool).await.unwrap();
    let p = parent(&pool, "p").await;
    let child = row(&pool, "children", "parent_id", p.id).await;
    row(&pool, "bare_children", "parent_id", p.id).await;

    assert!(p.soft_delete(&pool).await.is_err());
    assert_eq!(deleted_at(&pool, "parents", p.id).await, None, "the parent is not marked");
    assert_eq!(deleted_at(&pool, "children", child).await, None, "nothing else is marked");
}
