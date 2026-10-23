//! S3 (docs/TPD-billing.md): what a line and an invoice come to. The base (quantity times unit price, less the
//! discount), the tax on that base (the sum of the tax percents) and their total are each rounded to the currency's
//! minor unit, half up; the invoice is the sum of its lines.

use megh::billing::totals::{invoice, line, Amounts, TotalsError};
use megh::billing::value::{Currency, Micros, Percent, Quantity};
use rust_decimal::Decimal;

fn currency(code: &str) -> Currency {
    Currency::try_new(code).unwrap()
}

fn quantity(text: &str) -> Quantity {
    Quantity::try_new(text.parse::<Decimal>().unwrap()).unwrap()
}

fn percent(text: &str) -> Percent {
    Percent::try_new(text.parse::<Decimal>().unwrap()).unwrap()
}

/// Money written as units, e.g. 9.99, in micros.
fn money(units: &str) -> Micros {
    Micros::new((units.parse::<Decimal>().unwrap() * Decimal::from(1_000_000)).trunc().to_string().parse().unwrap())
}

fn amounts(sub_total: &str, tax_total: &str, total: &str) -> Amounts {
    Amounts { sub_total: money(sub_total), tax_total: money(tax_total), total: money(total) }
}

#[test]
fn a_line_without_discount_or_tax_is_quantity_times_price() {
    assert_eq!(line(&currency("USD"), quantity("2"), money("10.00"), percent("0"), &[]).unwrap(), amounts("20.00", "0", "20.00"));
    assert_eq!(line(&currency("USD"), quantity("1.5"), money("80.00"), Percent::default(), &[]).unwrap(), amounts("120.00", "0", "120.00"));
    assert_eq!(line(&currency("USD"), quantity("0"), money("80.00"), percent("0"), &[percent("7.5")]).unwrap(), amounts("0", "0", "0"), "nothing, nothing taxed");
}

#[test]
fn the_discount_comes_off_the_base_and_the_tax_is_on_what_is_left() {
    let taxed = line(&currency("USD"), quantity("3"), money("9.99"), percent("10"), &[percent("7.5")]).unwrap();
    assert_eq!(taxed, amounts("26.97", "2.02", "28.99"), "29.97 less 10% is 26.973; 7.5% of 26.97 is 2.02275");
}

#[test]
fn several_taxes_add_their_percents() {
    let taxed = line(&currency("USD"), quantity("1"), money("200.00"), percent("0"), &[percent("5"), percent("2.5")]).unwrap();
    assert_eq!(taxed, amounts("200.00", "15.00", "215.00"));
}

#[test]
fn halves_round_up() {
    assert_eq!(line(&currency("USD"), quantity("1"), money("1.005"), percent("0"), &[]).unwrap(), amounts("1.01", "0", "1.01"), "banker's rounding would give 1.00");
    assert_eq!(line(&currency("USD"), quantity("1"), money("1.00"), percent("0"), &[percent("12.5")]).unwrap(), amounts("1.00", "0.13", "1.13"), "0.125 is 0.13");
    assert_eq!(line(&currency("USD"), quantity("1"), money("1.00"), percent("0"), &[percent("2.4")]).unwrap().tax_total, money("0.02"), "0.024 is 0.02");
}

#[test]
fn the_minor_unit_is_the_currencys() {
    assert_eq!(line(&currency("JPY"), quantity("1"), money("1000.5"), percent("0"), &[percent("10")]).unwrap(), amounts("1001", "100", "1101"), "no minor unit: 1000.5 is 1001 and 10% of it 100.1 is 100");
    assert_eq!(line(&currency("KWD"), quantity("1"), money("1.2345"), percent("0"), &[]).unwrap(), amounts("1.235", "0", "1.235"), "three decimals");
}

#[test]
fn an_amount_too_large_to_keep_is_refused() {
    let huge = Micros::new(9_000_000_000_000_000_000);
    assert!(matches!(line(&currency("USD"), quantity("1000000000"), huge, percent("0"), &[]), Err(TotalsError::TooLarge)), "does not fit an i64 of micros");
    assert!(matches!(line(&currency("USD"), quantity("1000000000000000"), huge, percent("0"), &[]), Err(TotalsError::TooLarge)), "does not fit a decimal");
}

#[test]
fn an_invoice_is_the_sum_of_its_lines_and_adds_up() {
    let lines = [amounts("26.97", "2.02", "28.99"), amounts("80.00", "0", "80.00"), amounts("0.01", "0.01", "0.02")];
    let sum = invoice(&lines);
    assert_eq!(sum, amounts("106.98", "2.03", "109.01"));
    assert_eq!(i64::from(sum.sub_total) + i64::from(sum.tax_total), i64::from(sum.total));
    assert_eq!(invoice(&[]), amounts("0", "0", "0"));
}
