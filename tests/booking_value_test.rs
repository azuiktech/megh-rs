//! B1 (docs/TPD-booking.md): shared value types cannot be constructed invalid, and read and write as the strings stored.

use megh::booking::value::{Address, Attributes, Contact, Gender, Name, Person, Phone};
use serde_json::{from_value, json, to_value};

#[test]
fn name_is_trimmed_and_never_empty_or_over_255_chars() {
    assert_eq!(Name::try_new("  Dr Albert  ").unwrap().as_ref(), "Dr Albert");
    assert!(Name::try_new("   ").is_err());
    assert!(Name::try_new("é".repeat(255)).is_ok());
    assert!(Name::try_new("é".repeat(256)).is_err());
    assert!(from_value::<Name>(json!("")).is_err());
}

#[test]
fn phone_is_stored_in_e164_and_must_be_valid() {
    assert_eq!(Phone::try_new("+1 (516) 555-0100").unwrap().as_ref(), "+15165550100");
    assert_eq!(to_value(Phone::try_new("+44 20 7946 0958").unwrap()).unwrap(), json!("+442079460958"));
    assert!(Phone::try_new("5165550100").is_err(), "no country code");
    assert!(Phone::try_new("+1 123").is_err());
    assert!(from_value::<Phone>(json!("not a phone")).is_err());
}

#[test]
fn gender_is_snake_case_text() {
    assert_eq!(to_value(Gender::Female).unwrap(), json!("female"));
    assert_eq!(from_value::<Gender>(json!("other")).unwrap(), Gender::Other);
    assert!(from_value::<Gender>(json!("unknown")).is_err());
}

#[test]
fn attributes_are_a_json_object_only() {
    let attributes: Attributes = from_value(json!({"specialty": "hygienist", "years": 12})).unwrap();
    assert_eq!(attributes["years"], json!(12));
    assert!(from_value::<Attributes>(json!(["hygienist"])).is_err());
    assert!(from_value::<Attributes>(json!("hygienist")).is_err());
}

#[test]
fn address_round_trips_with_an_alpha2_country() {
    let stored = json!({
        "line_1": "333 Bush Street", "line_2": null, "city": "San Francisco",
        "region": "CA", "postal_code": "94104", "country": "US"
    });
    let address: Address = from_value(stored.clone()).unwrap();
    assert_eq!(to_value(&address).unwrap(), stored);
    assert!(from_value::<Address>(json!({
        "line_1": "333 Bush Street", "line_2": null, "city": "San Francisco",
        "region": "CA", "postal_code": "94104", "country": "XX"
    })).is_err());
}

#[test]
fn contact_fields_are_each_optional_but_valid_when_present() {
    let contact: Contact = from_value(json!({
        "email": "ada@example.com", "home_phone": null, "work_phone": null, "mobile_phone": "+1 516 555 0100"
    })).unwrap();
    assert_eq!(to_value(&contact).unwrap()["mobile_phone"], json!("+15165550100"));
    assert!(from_value::<Contact>(json!({
        "email": "not-an-email", "home_phone": null, "work_phone": null, "mobile_phone": null
    })).is_err());
}

#[test]
fn person_requires_names_gender_and_birth_date() {
    let person: Person = from_value(json!({
        "first_name": "Ada", "last_name": "Lovelace", "gender": "female", "birth_date": "1815-12-10"
    })).unwrap();
    assert_eq!(person.first_name.as_ref(), "Ada");
    assert!(from_value::<Person>(json!({"first_name": "Ada", "last_name": "Lovelace", "gender": "female"})).is_err());
    assert!(from_value::<Person>(json!({
        "first_name": " ", "last_name": "Lovelace", "gender": "female", "birth_date": "1815-12-10"
    })).is_err());
}
