//! Value types shared by booking entities. None can be constructed invalid.

use celes::Country;
use chrono::NaiveDate;
use email_address::EmailAddress;
use nutype::nutype;
use serde::{Deserialize, Serialize};

/// Assorted metadata on every entity: a JSON object.
pub type Attributes = serde_json::Map<String, serde_json::Value>;

#[nutype(
    sanitize(trim),
    validate(not_empty, len_char_max = 255),
    derive(Debug, Clone, PartialEq, Eq, Hash, AsRef, Display, TryFrom, Serialize, Deserialize),
)]
pub struct Name(String);

/// An international phone number, kept in E.164 (`+15165550100`).
#[nutype(
    sanitize(with = |s: String| phonenumber::parse(None, &s).map_or(s, |n| n.format().mode(phonenumber::Mode::E164).to_string())),
    validate(predicate = |s: &str| phonenumber::parse(None, s).is_ok_and(|n| n.is_valid())),
    derive(Debug, Clone, PartialEq, Eq, Hash, AsRef, Display, TryFrom, Serialize, Deserialize),
)]
pub struct Phone(String);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Gender {
    Male,
    Female,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Address {
    pub line_1: Name,
    pub line_2: Option<Name>,
    pub city: Name,
    pub region: Name,
    pub postal_code: Name,
    pub country: Country,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Contact {
    pub email: Option<EmailAddress>,
    pub home_phone: Option<Phone>,
    pub work_phone: Option<Phone>,
    pub mobile_phone: Option<Phone>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Person {
    pub first_name: Name,
    pub last_name: Name,
    pub gender: Gender,
    pub birth_date: NaiveDate,
}
