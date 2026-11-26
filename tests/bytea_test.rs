//! A `Vec<u8>` field is a bytea column: the bytes written are the bytes stored and read.

#[macro_use]
mod common;

use megh::auth::crud;
use megh::entity;
use serde::{Deserialize, Serialize};
use megh::Db;
use uuid::Uuid;
use validator::Validate;

#[entity(table = "files", keys = ["id"], timestamps, soft_delete)]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Validate)]
struct File {
    id: Uuid,
    name: String,
    content: Vec<u8>,
}

const PDF: &[u8] = b"%PDF-1.4\n\x00\xff\x80 binary\n%%EOF";

async fn table(pool: &Db) {
    Db::query("CREATE TABLE files (id UUID PRIMARY KEY, name TEXT NOT NULL, content BYTEA NOT NULL, created_at TIMESTAMPTZ NOT NULL DEFAULT now(), updated_at TIMESTAMPTZ NOT NULL DEFAULT now(), deleted_at TIMESTAMPTZ)").sqlite("CREATE TABLE files (id UUID PRIMARY KEY, name TEXT NOT NULL, content BYTEA NOT NULL, created_at TIMESTAMPTZ NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%f+00:00')), updated_at TIMESTAMPTZ NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%f+00:00')), deleted_at TIMESTAMPTZ)")
        .execute(pool)
        .await
        .unwrap();
}

fn file(content: &[u8]) -> File {
    serde_json::from_value(serde_json::json!({"id": Uuid::nil(), "name": "po.pdf", "content": content})).unwrap()
}

dual!(created_and_replaced_bytes_are_the_columns_bytes);
async fn created_and_replaced_bytes_are_the_columns_bytes(pool: Db) {
    table(&pool).await;

    let created = crud::create(&pool, file(PDF), None).await.unwrap();
    let column: Vec<u8> = Db::query_scalar("SELECT content FROM files").fetch_one(&pool).await.unwrap();
    assert_eq!((column.as_slice(), created.content.as_slice()), (PDF, PDF));

    let replaced = crud::replace(&pool, File { content: b"\x01\x02".to_vec(), ..created }, None).await.unwrap();
    let column: Vec<u8> = Db::query_scalar("SELECT content FROM files").fetch_one(&pool).await.unwrap();
    assert_eq!((column.as_slice(), replaced.content.as_slice()), (&b"\x01\x02"[..], &b"\x01\x02"[..]));
}
