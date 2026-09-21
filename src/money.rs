//! Prices are `BIGINT` millionths of a unit in the database (megh-go's `MicroUnit`) and `Money` in Rust.

use rust_decimal::prelude::ToPrimitive;
use rust_decimal::Decimal;
pub use rusty_money::{iso, Money, MoneyError};

/// 1 unit of currency is this many database units: 1 USD = 1,000,000.
pub const MICROS_PER_UNIT: i64 = 1_000_000;

/// The price stored as `micros` in the currency with that ISO 4217 code.
pub fn from_micros(micros: i64, currency: &str) -> Result<Money<'static, iso::Currency>, MoneyError> {
    let currency = iso::find(currency).ok_or(MoneyError::InvalidCurrency)?;
    Ok(Money::from_decimal(Decimal::from(micros) / Decimal::from(MICROS_PER_UNIT), currency))
}

/// The value to store, failing rather than rounding when the amount has more than 6 decimals.
pub fn to_micros(money: &Money<iso::Currency>) -> Result<i64, MoneyError> {
    let micros = money.amount().checked_mul(Decimal::from(MICROS_PER_UNIT)).ok_or(MoneyError::Overflow)?;
    match micros.fract().is_zero() {
        true => micros.to_i64().ok_or(MoneyError::Overflow),
        false => Err(MoneyError::PrecisionLoss),
    }
}
