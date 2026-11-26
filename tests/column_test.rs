//! docs/TPD-persistence.md: the metadata `#[entity]` generates, and a column's value from JSON through the field's type.

use megh::auth::audit::AuditPolicy;
use megh::booking::client::Client;
use megh::booking::schedule::Schedule;
use megh::Table;
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;
use validator::Validate;

fn sorted(mut names: Vec<&'static str>) -> Vec<&'static str> {
    names.sort();
    names
}

#[test]
fn client_metadata() {
    assert_eq!(Client::TABLE_NAME, "clients");
    assert_eq!(Client::PRIMARY_KEY, ["tenant_id", "id"]);
    assert!(Client::SOFT_DELETE);
    assert_eq!(Client::AUDIT, AuditPolicy::Shared);
    assert_eq!(
        sorted(Client::COLUMNS.to_vec()),
        [
            "address", "attributes", "birth_date", "created_at", "deleted_at", "email", "first_name", "gender", "home_phone", "id",
            "last_name", "mobile_phone", "name", "role", "tenant_id", "updated_at", "work_phone",
        ],
        "the entity's columns, the flattened contact's, and the injected timestamps; nothing else"
    );
    assert!(Client::LISTS.is_empty());
}

#[test]
fn a_field_stored_as_an_array_is_a_list() {
    assert_eq!(Schedule::LISTS, ["service_ids"]);
}

#[megh::entity(table = "plains", keys = ["tenant_id", "plain_id"])]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Validate)]
struct Plain {
    tenant_id: Uuid,
    plain_id: Uuid,
    body: String,
}

#[test]
fn nothing_is_injected_unless_asked_for() {
    assert_eq!(Plain::COLUMNS, ["tenant_id", "plain_id", "body"]);
    assert_eq!(Plain::PRIMARY_KEY, ["tenant_id", "plain_id"]);
    assert!(!Plain::SOFT_DELETE);
    assert_eq!(Plain::AUDIT, AuditPolicy::Disabled);
}

#[test]
fn a_value_from_json_follows_the_fields_type() {
    assert!(Client::param("first_name", &json!("Ada")).is_ok());
    assert!(Client::param("first_name", &json!("")).is_err(), "the newtype's own rules apply");
    assert!(Client::param("gender", &json!("female")).is_ok());
    assert!(Client::param("gender", &json!("alien")).is_err());
    assert!(Client::param("birth_date", &json!("1815-12-10")).is_ok());
    assert!(Client::param("birth_date", &json!("not a date")).is_err());
    assert!(Client::param("email", &json!("ada@example.com")).is_ok(), "a flattened field");
    assert!(Schedule::param("service_ids", &json!([Uuid::now_v7()])).is_ok(), "a list, through its array");
    assert!(Client::param("nickname", &json!("x")).is_err(), "no such column");
}
