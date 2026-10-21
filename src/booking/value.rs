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
    cfg_attr(feature = "postgres", derive_unchecked(sqlx::Type)),
    cfg_attr(feature = "axum", derive_unchecked(utoipa::ToSchema)),
)]
#[cfg_attr(feature = "postgres", sqlx(transparent))]
pub struct Name(String);

/// An international phone number, kept in E.164 (`+15165550100`).
#[nutype(
    sanitize(with = |s: String| phonenumber::parse(None, &s).map_or(s, |n| n.format().mode(phonenumber::Mode::E164).to_string())),
    validate(predicate = |s: &str| phonenumber::parse(None, s).is_ok_and(|n| n.is_valid())),
    derive(Debug, Clone, PartialEq, Eq, Hash, AsRef, Display, TryFrom, Serialize, Deserialize),
    cfg_attr(feature = "postgres", derive_unchecked(sqlx::Type)),
    cfg_attr(feature = "axum", derive_unchecked(utoipa::ToSchema)),
)]
#[cfg_attr(feature = "postgres", sqlx(transparent))]
pub struct Phone(String);

/// An email address, checked by `email_address`.
#[nutype(
    sanitize(trim),
    validate(predicate = |s: &str| EmailAddress::is_valid(s)),
    derive(Debug, Clone, PartialEq, Eq, Hash, AsRef, Display, TryFrom, Serialize, Deserialize),
    cfg_attr(feature = "postgres", derive_unchecked(sqlx::Type)),
    cfg_attr(feature = "axum", derive_unchecked(utoipa::ToSchema)),
)]
#[cfg_attr(feature = "postgres", sqlx(transparent))]
pub struct Email(String);

/// How many appointments fit at the same time; double booking is a capacity of 2.
#[nutype(
    validate(greater_or_equal = 1),
    default = 1,
    derive(Debug, Clone, Copy, PartialEq, Eq, Default, TryFrom, Into, Serialize, Deserialize),
)]
pub struct Capacity(i32);

/// How many appointments may wait for a place.
#[nutype(
    validate(greater_or_equal = 0),
    default = 0,
    derive(Debug, Clone, Copy, PartialEq, Eq, Default, TryFrom, Into, Serialize, Deserialize),
)]
pub struct Waitlist(i32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "postgres", derive(sqlx::Type))]
#[cfg_attr(feature = "postgres", sqlx(type_name = "text", rename_all = "snake_case"))]
#[cfg_attr(feature = "axum", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum Gender {
    Male,
    Female,
    Other,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "axum", derive(utoipa::ToSchema))]
pub struct Address {
    pub line_1: Name,
    pub line_2: Option<Name>,
    pub city: Name,
    pub region: Name,
    pub postal_code: Name,
    #[cfg_attr(feature = "axum", schema(value_type = String, example = "US"))]
    pub country: Country,
}

/// Columns of a person's contact, flattened into the table of the entity that has them.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, crate::Table)]
#[cfg_attr(feature = "postgres", derive(sqlx::FromRow))]
#[cfg_attr(feature = "axum", derive(utoipa::ToSchema))]
pub struct Contact {
    pub email: Option<Email>,
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
