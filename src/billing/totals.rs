//! What a line and an invoice come to. A line is quantity times unit price, less the discount percent, plus the tax
//! percentages of its taxes; the base, the tax and the total of each line are rounded to the currency's minor unit
//! (cents), half up, with `rusty_money`'s `Money::round`. The invoice is the sum of its lines, so it adds up exactly.

use rust_decimal::Decimal;
use rusty_money::{iso, Money, MoneyError, Round};

use crate::money::to_micros;

use super::value::{Currency, Micros, Percent, Quantity};

/// A line's or an invoice's amounts, in micros.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Amounts {
    pub sub_total: Micros,
    pub tax_total: Micros,
    pub total: Micros,
}

#[derive(Debug, thiserror::Error)]
pub enum TotalsError {
    #[error(transparent)]
    Currency(#[from] MoneyError),
    #[error("the amount is too large")]
    TooLarge,
}

/// One line: the base after the discount, the tax on it, and their sum.
pub fn line(currency: &Currency, quantity: Quantity, unit_price: Micros, discount: Percent, taxes: &[Percent]) -> Result<Amounts, TotalsError> {
    let currency = iso::find(currency.as_ref()).ok_or(MoneyError::InvalidCurrency)?;
    let base = Decimal::new(i64::from(unit_price), 6)
        .checked_mul(Decimal::from(quantity))
        .and_then(|price| price.checked_mul(Decimal::ONE_HUNDRED - Decimal::from(discount)))
        .map(|price| price / Decimal::ONE_HUNDRED)
        .ok_or(TotalsError::TooLarge)?;
    let sub_total = round(base, currency)?;
    let rate: Decimal = taxes.iter().copied().map(Decimal::from).sum();
    let tax = Decimal::new(i64::from(sub_total), 6).checked_mul(rate).ok_or(TotalsError::TooLarge)? / Decimal::ONE_HUNDRED;
    let tax_total = round(tax, currency)?;
    let total = i64::from(sub_total).checked_add(i64::from(tax_total)).ok_or(TotalsError::TooLarge)?;
    Ok(Amounts { sub_total, tax_total, total: Micros::new(total) })
}

/// The amount in the currency's minor unit, half up, as micros.
fn round(amount: Decimal, currency: &iso::Currency) -> Result<Micros, TotalsError> {
    let money = Money::from_decimal(amount, currency).round(currency.exponent, Round::HalfUp);
    match to_micros(&money) {
        Ok(micros) => Ok(Micros::new(micros)),
        Err(MoneyError::Overflow) => Err(TotalsError::TooLarge),
        Err(error) => Err(error.into()),
    }
}

/// An invoice: the sums of its lines.
pub fn invoice(lines: &[Amounts]) -> Amounts {
    let sum = |amount: fn(&Amounts) -> Micros| Micros::new(lines.iter().map(|line| i64::from(amount(line))).fold(0, i64::saturating_add));
    Amounts { sub_total: sum(|line| line.sub_total), tax_total: sum(|line| line.tax_total), total: sum(|line| line.total) }
}
