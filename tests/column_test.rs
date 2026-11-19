//! docs/TPD-persistence.md: the type mapping (`Column`) and the metadata `#[entity]` generates.

use std::fmt::Debug;

use chrono::{DateTime, NaiveDate, Utc};
use megh::auth::audit::AuditPolicy;
use megh::booking::client::Client;
use megh::booking::value::{Email, Gender, Name};
use megh::auth::column::{Column, Kind};
use megh::sea_query::Value;
use megh::Table;
use serde::{Deserialize, Serialize};
use serde_json::json;
use uuid::Uuid;
use validator::Validate;

fn round_trips<T: Column + PartialEq + Debug>(value: T) {
    assert_eq!(T::from_value(value.to_value()).unwrap(), value);
}

#[test]
fn values_round_trip() {
    round_trips("text".to_string());
    round_trips(true);
    round_trips(7i32);
    round_trips(7i64);
    round_trips(2.5f64);
    round_trips(Uuid::now_v7());
    round_trips(NaiveDate::from_ymd_opt(1815, 12, 10).unwrap());
    round_trips(Utc::now());
    round_trips(Some(5i64));
    round_trips(None::<i64>);
    round_trips(Name::try_new("Ada").unwrap());
    round_trips(Some(Email::try_new("ada@example.com").unwrap()));
    round_trips(None::<Email>);
    [Gender::Male, Gender::Female, Gender::Other, Gender::Unknown].into_iter().for_each(round_trips);
}

#[test]
fn kinds() {
    assert!(matches!(<String as Column>::KIND, Kind::Text));
    assert!(matches!(<Name as Column>::KIND, Kind::Text));
    assert!(matches!(<Gender as Column>::KIND, Kind::Text));
    assert!(matches!(<bool as Column>::KIND, Kind::Bool));
    assert!(matches!(<i32 as Column>::KIND, Kind::Int));
    assert!(matches!(<i64 as Column>::KIND, Kind::Int));
    assert!(matches!(<Uuid as Column>::KIND, Kind::Uuid));
    assert!(matches!(<NaiveDate as Column>::KIND, Kind::Date));
    assert!(matches!(<DateTime<Utc> as Column>::KIND, Kind::Timestamp));
    assert!(matches!(<Option<Uuid> as Column>::KIND, Kind::Uuid), "an option has the kind of its value");
}

#[test]
fn values_that_do_not_fit_are_errors() {
    assert!(i64::from_value(Value::from("x".to_string())).is_err());
    assert!(Uuid::from_value(Value::Uuid(None)).is_err(), "null is not a Uuid");
    assert!(Name::from_value(Value::from(String::new())).is_err(), "the newtype's own rules apply");
    assert!(Gender::from_value(Value::from("alien".to_string())).is_err());
    assert_eq!(Option::<i64>::from_value(Value::BigInt(None)).unwrap(), None);
}

fn names<T: Table>() -> Vec<&'static str> {
    T::COLUMNS.iter().map(|column| column.name).collect()
}

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
        sorted(names::<Client>()),
        [
            "address", "attributes", "birth_date", "created_at", "deleted_at", "email", "first_name", "gender", "home_phone", "id",
            "last_name", "mobile_phone", "name", "role", "tenant_id", "updated_at", "work_phone",
        ],
        "the entity's columns, the flattened contact's, and the injected timestamps; nothing else"
    );
    let kind = |name: &str| Client::COLUMNS.iter().find(|column| column.name == name).unwrap().kind;
    assert!(matches!(kind("id"), Kind::Uuid));
    assert!(matches!(kind("birth_date"), Kind::Date));
    assert!(matches!(kind("address"), Kind::Json) && matches!(kind("attributes"), Kind::Json));
    assert!(matches!(kind("created_at"), Kind::Timestamp) && matches!(kind("deleted_at"), Kind::Timestamp));
    assert!(matches!(kind("gender"), Kind::Text) && matches!(kind("email"), Kind::Text));
    let keys: Vec<&str> = Client::COLUMNS.iter().filter(|column| column.is_primary_key).map(|column| column.name).collect();
    assert_eq!(sorted(keys), ["id", "tenant_id"]);
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
    assert_eq!(names::<Plain>(), ["tenant_id", "plain_id", "body"]);
    assert_eq!(Plain::PRIMARY_KEY, ["tenant_id", "plain_id"]);
    assert!(!Plain::SOFT_DELETE);
    assert_eq!(Plain::AUDIT, AuditPolicy::Disabled);
}

#[test]
fn values_follow_the_columns_and_rebuild_the_entity() {
    let client: Client = serde_json::from_value(json!({
        "tenant_id": Uuid::now_v7(), "id": Uuid::now_v7(), "first_name": "Ada", "last_name": "Lovelace", "name": "Ada",
        "gender": "female", "birth_date": "1815-12-10", "email": "ada@example.com", "role": "mathematician",
        "attributes": {"vip": true},
    }))
    .unwrap();
    let values = client.values();
    assert_eq!(values.iter().map(|(name, _)| *name).collect::<Vec<_>>(), names::<Client>());
    let rebuilt = Client::from_values(values.into_iter().map(|(_, value)| value).collect()).unwrap();
    assert_eq!(rebuilt, client);
}
