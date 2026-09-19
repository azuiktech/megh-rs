//! Unit tests for #[derive(Table)] procedural macro and metadata extraction.

use megh::{ColumnMeta, Table};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Table)]
struct DefaultEntity {
    id: i32,
    name: String,
}

#[derive(Debug, Serialize, Deserialize, Table)]
#[table(name = "user_accounts", keys = ["account_id", "provider"])]
#[sqlx(rename_all = "snake_case")]
struct CompositeKeyEntity {
    account_id: String,
    provider: String,
    #[sqlx(rename = "mail_address")]
    email: Option<String>,
    #[sqlx(json)]
    payload: serde_json::Value,
    #[sqlx(default)]
    status: String,
    #[sqlx(skip)]
    temp_token: Option<String>,
}

#[test]
fn test_default_table_name_and_primary_key() {
    assert_eq!(DefaultEntity::TABLE_NAME, "default_entity");
    assert_eq!(DefaultEntity::PRIMARY_KEY, &["id"]);
    assert_eq!(DefaultEntity::COLUMNS.len(), 2);
    assert_eq!(DefaultEntity::COLUMNS[0].name, "id");
    assert!(DefaultEntity::COLUMNS[0].is_primary_key);
    assert_eq!(DefaultEntity::COLUMNS[1].name, "name");
    assert!(!DefaultEntity::COLUMNS[1].is_primary_key);
}

#[test]
fn test_composite_key_and_sqlx_attributes() {
    assert_eq!(CompositeKeyEntity::TABLE_NAME, "user_accounts");
    assert_eq!(CompositeKeyEntity::PRIMARY_KEY, &["account_id", "provider"]);

    // Skipped field must be excluded
    let col_names: Vec<&str> = CompositeKeyEntity::COLUMNS.iter().map(|c| c.name).collect();
    assert!(!col_names.contains(&"temp_token"));

    // Renamed field must use custom column name
    assert!(col_names.contains(&"mail_address"));
    assert!(!col_names.contains(&"email"));

    // Check metadata flags
    let payload_col = CompositeKeyEntity::COLUMNS.iter().find(|c| c.name == "payload").unwrap();
    assert!(payload_col.is_json);

    let status_col = CompositeKeyEntity::COLUMNS.iter().find(|c| c.name == "status").unwrap();
    assert!(status_col.has_default);

    let update_cols = CompositeKeyEntity::update_columns();
    assert!(!update_cols.contains(&"account_id"));
    assert!(!update_cols.contains(&"provider"));
    assert!(update_cols.contains(&"mail_address"));
    assert!(update_cols.contains(&"payload"));
    assert!(update_cols.contains(&"status"));
}
