//! Specification: docs/TPD-booking.md, R5 and https://google.aip.dev/160: a condition is a filter expression over the
//! key/value attributes of an entity.

use megh::booking::{Attributes, Condition, Error, Filter, Predicate};
use serde_json::{json, Value};

fn subject(attributes: Value) -> Attributes {
    attributes.as_object().unwrap().iter().map(|(key, value)| (key.clone(), value.clone())).collect()
}

fn holds(expression: &str, attributes: Value) -> bool {
    Filter::parse(expression).unwrap().holds(&subject(attributes))
}

#[test]
fn a_string_equals_or_does_not() {
    assert!(holds(r#"role = "stylist""#, json!({"role": "stylist"})));
    assert!(!holds(r#"role = "stylist""#, json!({"role": "dentist"})));
    assert!(holds(r#"role != "stylist""#, json!({"role": "dentist"})));
}

#[test]
fn a_missing_field_fails_every_restriction_even_not_equal() {
    assert!(!holds(r#"role = "stylist""#, json!({})));
    assert!(!holds(r#"role != "stylist""#, json!({})));
}

#[test]
fn numbers_compare_as_numbers() {
    assert!(holds("experience_years >= 10", json!({"experience_years": 12})));
    assert!(!holds("experience_years >= 10", json!({"experience_years": 9})));
    assert!(holds("experience_years < 11", json!({"experience_years": 10.5})));
}

#[test]
fn booleans_compare_by_value() {
    assert!(holds("accessible = true", json!({"accessible": true})));
    assert!(!holds("accessible = true", json!({"accessible": false})));
}

#[test]
fn a_star_in_a_string_matches_anything_and_a_dot_is_a_dot() {
    assert!(holds(r#"name = "Dr*""#, json!({"name": "Dr Albert"})));
    assert!(holds(r#"name = "*bert""#, json!({"name": "Dr Albert"})));
    assert!(!holds(r#"name = "Dr.*""#, json!({"name": "Dr Albert"})));
}

#[test]
fn and_or_and_not_combine_restrictions() {
    let master = json!({"role": "stylist", "level": "master"});
    assert!(holds(r#"role = "stylist" AND level = "master""#, master.clone()));
    assert!(holds(r#"role = "dentist" OR level = "master""#, master.clone()));
    assert!(holds(r#"NOT level = "junior""#, master.clone()));
    assert!(holds(r#"-level = "junior""#, master.clone()));
    assert!(!holds(r#"NOT (role = "stylist")"#, master));
}

#[test]
fn or_binds_tighter_than_and() {
    assert!(holds("a = 1 AND b = 2 OR c = 3", json!({"a": 1, "b": 0, "c": 3})));
    assert!(!holds("a = 1 AND b = 2 OR c = 3", json!({"a": 0, "b": 0, "c": 3})));
}

#[test]
fn restrictions_side_by_side_are_an_implicit_and() {
    assert!(holds(r#"role = "stylist" level = "master""#, json!({"role": "stylist", "level": "master"})));
    assert!(!holds(r#"role = "stylist" level = "master""#, json!({"role": "stylist", "level": "junior"})));
}

#[test]
fn parentheses_group() {
    let junior = json!({"role": "dentist", "level": "junior", "open": true});
    assert!(holds(r#"(role = "stylist" OR role = "dentist") AND level = "junior""#, junior));
}

#[test]
fn a_dot_goes_into_a_nested_object() {
    assert!(holds(r#"address.city = "Paris""#, json!({"address": {"city": "Paris"}})));
    assert!(!holds(r#"address.city = "Paris""#, json!({"address": {"city": "Rome"}})));
    assert!(!holds(r#"address.city = "Paris""#, json!({"address": "Paris"})));
}

#[test]
fn has_looks_in_arrays_objects_and_scalars() {
    assert!(holds(r#"languages:"fr""#, json!({"languages": ["en", "fr"]})));
    assert!(!holds(r#"languages:"de""#, json!({"languages": ["en", "fr"]})));
    assert!(holds("address:city", json!({"address": {"city": "Paris"}})));
    assert!(holds("address:*", json!({"address": {"city": "Paris"}})));
    assert!(!holds("address:*", json!({"address": {}})));
    assert!(holds(r#"role:"stylist""#, json!({"role": "stylist"})));
}

#[test]
fn timestamps_compare_in_time() {
    assert!(holds(r#"valid_until > "2026-01-01T00:00:00Z""#, json!({"valid_until": "2026-06-01T09:00:00-04:00"})));
    assert!(!holds(r#"valid_until > "2027-01-01T00:00:00Z""#, json!({"valid_until": "2026-06-01T09:00:00-04:00"})));
}

#[test]
fn a_bare_value_matches_anywhere_in_the_attributes() {
    assert!(holds(r#""master""#, json!({"level": "master"})));
    assert!(!holds(r#""grand""#, json!({"level": "master"})));
}

#[test]
fn an_empty_filter_holds_for_everything() {
    assert!(holds("", json!({"role": "stylist"})));
    assert!(holds("", json!({})));
}

#[test]
fn what_the_parser_would_mis_read_is_refused() {
    for text in ["lower(role) = 1", "role=\"stylist\"", "NOT role=\"stylist\"", "a =", "a = = b", "rating > 4.5", "a = b.c"] {
        assert!(matches!(Filter::parse(text), Err(Error::Invalid(_))), "{text:?}");
    }
}

#[test]
fn malformed_text_is_invalid_and_never_panics() {
    for text in ["(", "= a", "AND", "a AND", "((a)", "a OR OR b", "\"unterminated", "a.b.", ".."] {
        let parsed = std::panic::catch_unwind(|| Filter::parse(text));
        assert!(matches!(parsed, Ok(Err(Error::Invalid(_)))), "{text:?}");
    }
}

#[test]
fn a_filter_is_stored_and_read_back_as_a_condition() {
    let stored = serde_json::to_value(Box::new(Filter::parse(r#"role = "stylist""#).unwrap()) as Condition).unwrap();
    assert_eq!(stored, json!({"Filter": r#"role = "stylist""#}));
    let read: Condition = serde_json::from_value(stored).unwrap();
    assert!(read.holds(&subject(json!({"role": "stylist"}))));
}
