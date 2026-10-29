//! A bytea column of an entity holds the field's bytes through `insert`, `save` and reads: `megh::Bytea`.

use megh::{Bytea, Entity, Table};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, sqlx::FromRow, Table)]
#[table(name = "files", keys = ["id"])]
struct FileData {
    name: String,
    content: Bytea,
}

const PDF: &[u8] = b"%PDF-1.4\n\x00\xff\x80 binary\n%%EOF";

async fn table(pool: &PgPool) {
    sqlx::query("CREATE TABLE files (id UUID PRIMARY KEY, name TEXT NOT NULL, content BYTEA NOT NULL, created_at TIMESTAMPTZ NOT NULL DEFAULT now(), updated_at TIMESTAMPTZ NOT NULL DEFAULT now(), deleted_at TIMESTAMPTZ)")
        .execute(pool)
        .await
        .unwrap();
}

#[sqlx::test(migrations = false)]
async fn inserted_and_saved_bytes_are_the_columns_bytes(pool: PgPool) {
    table(&pool).await;

    let inserted = Entity::new(Uuid::now_v7(), FileData { name: "po.pdf".into(), content: Bytea(PDF.to_vec()) }).insert(&pool).await.unwrap();
    let column: Vec<u8> = sqlx::query_scalar("SELECT content FROM files").fetch_one(&pool).await.unwrap();
    assert_eq!((column.as_slice(), inserted.data.content.0.as_slice()), (PDF, PDF));

    let saved = inserted.map(|data| FileData { content: Bytea(b"\x01\x02".to_vec()), ..data }).save(&pool).await.unwrap();
    let column: Vec<u8> = sqlx::query_scalar("SELECT content FROM files").fetch_one(&pool).await.unwrap();
    assert_eq!((column.as_slice(), saved.data.content.0.as_slice()), (&b"\x01\x02"[..], &b"\x01\x02"[..]));
}

#[test]
fn bytes_are_postgres_hex_text_in_json_and_back() {
    let json = serde_json::to_value(Bytea(vec![0x25, 0x00, 0xff])).unwrap();

    assert_eq!(json, serde_json::json!("\\x2500ff"));
    assert_eq!(serde_json::from_value::<Bytea>(json).unwrap(), Bytea(vec![0x25, 0x00, 0xff]));
}
