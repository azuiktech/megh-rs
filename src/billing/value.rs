//! Values of the billing documents. None can be constructed invalid.

use nutype::nutype;
use rust_decimal::Decimal;
use rusty_money::iso;
use serde::{Deserialize, Serialize};

/// Money in millionths of the currency of its document, as the other billing tables store it
/// (`money::from_micros` gives the `Money`).
#[nutype(
    default = 0,
    derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, From, Into, Serialize, Deserialize),
    cfg_attr(feature = "sql", derive_unchecked(sqlx::Type)),
    cfg_attr(feature = "axum", derive_unchecked(utoipa::ToSchema)),
)]
#[cfg_attr(feature = "sql", sqlx(transparent))]
pub struct Micros(i64);

/// An ISO 4217 code such as `USD`.
#[nutype(
    sanitize(with = |code: String| code.trim().to_uppercase()),
    validate(predicate = |code: &str| iso::find(code).is_some()),
    derive(Debug, Clone, PartialEq, Eq, Hash, AsRef, Display, TryFrom, Into, Serialize, Deserialize),
    cfg_attr(feature = "sql", derive_unchecked(sqlx::Type)),
    cfg_attr(feature = "axum", derive_unchecked(utoipa::ToSchema)),
)]
#[cfg_attr(feature = "sql", sqlx(transparent))]
pub struct Currency(String);

/// How many units of a product or service; not negative.
#[nutype(
    validate(predicate = |quantity: &Decimal| !quantity.is_sign_negative()),
    derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, TryFrom, Into, Serialize, Deserialize),
    cfg_attr(feature = "axum", derive_unchecked(utoipa::ToSchema)),
)]
#[cfg_attr(feature = "axum", schema(value_type = String, example = "1.5"))]
pub struct Quantity(Decimal);

/// A percentage from 0 to 100: a discount or a tax rate.
#[nutype(
    validate(predicate = |percent: &Decimal| *percent >= Decimal::ZERO && *percent <= Decimal::ONE_HUNDRED),
    default = Decimal::ZERO,
    derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, TryFrom, Into, Serialize, Deserialize),
    cfg_attr(feature = "axum", derive_unchecked(utoipa::ToSchema)),
)]
#[cfg_attr(feature = "axum", schema(value_type = String, example = "7.5"))]
pub struct Percent(Decimal);

/// How money moved: HubSpot's `hs_payment_method_type`, with its bank rails grouped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "sql", derive(sqlx::Type), sqlx(type_name = "text", rename_all = "snake_case"))]
#[cfg_attr(feature = "axum", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum PaymentMethod {
    Cash,
    Cheque,
    Card,
    BankTransfer,
    Other,
}


#[cfg(feature = "sql")]
crate::sqlx_type!(Quantity, match driver {
    Postgres => (
        |value: &Quantity| -> Decimal { value.into_inner() },
        |stored: Decimal| Ok(Quantity::try_new(stored)?),
    ),
    _ => (
        |value: &Quantity| -> String { value.into_inner().to_string() },
        |stored: String| Ok(Quantity::try_new(stored.parse::<Decimal>()?)?),
    ),
});

#[cfg(feature = "sql")]
crate::sqlx_type!(Percent, match driver {
    Postgres => (
        |value: &Percent| -> Decimal { value.into_inner() },
        |stored: Decimal| Ok(Percent::try_new(stored)?),
    ),
    _ => (
        |value: &Percent| -> String { value.into_inner().to_string() },
        |stored: String| Ok(Percent::try_new(stored.parse::<Decimal>()?)?),
    ),
});
