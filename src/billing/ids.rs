//! The ids of invoices and payments (`INV-2026-0001`). The app supplies an `Ids` function; its result is stored as a
//! `Name` like any other. `Numbering` is the ready-made format: prefix, separator, an optional date part and a
//! zero-padded sequence. A counter row per org, kind of document and scope numbers them; no format is stored.

use std::sync::Arc;

use chrono::NaiveDate;
use sqlx::PgConnection;
use uuid::Uuid;

use crate::booking::value::Name;

/// The longest a `Name` can be.
const NAME_LENGTH: usize = 255;

/// The documents that have an id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, sqlx::Type)]
#[sqlx(type_name = "text", rename_all = "snake_case")]
pub enum Numbered {
    Invoice,
    Payment,
}

/// How an id is made from the app's function.
pub trait Ids {
    /// Which run of numbers a date belongs to: "" for one endless run, "2026" for a yearly one, "20261022" for a daily one.
    /// A new scope starts again at 1.
    fn scope(&self, on: NaiveDate) -> String;

    /// The id of the `sequence`-th document of that scope.
    fn id(&self, on: NaiveDate, sequence: i64) -> Name;
}

/// The part of an id between the prefix and the sequence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Middle {
    None,
    /// `2026`
    Year,
    /// `202610`
    YearMonth,
    /// `20261022`
    Date,
}

#[derive(Debug, thiserror::Error)]
pub enum NumberingError {
    #[error("the prefix and separators leave no room for the sequence in 255 characters")]
    TooLong,
}

/// Prefix, separator, optional date part, separator, sequence: `Numbering::new("INV", "-", Middle::Date, 4)` makes
/// `INV-20261022-0001`; with `Middle::None` it makes `INV-0001`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Numbering {
    prefix: String,
    separator: String,
    middle: Middle,
    padding: usize,
}

impl Numbering {
    pub fn new(prefix: &str, separator: &str, middle: Middle, padding: usize) -> Result<Self, NumberingError> {
        let numbering = Self { prefix: prefix.into(), separator: separator.into(), middle, padding };
        (numbering.longest() <= NAME_LENGTH).then_some(numbering).ok_or(NumberingError::TooLong)
    }

    /// The most characters an id can have: a sequence of any `i64` is at most 19 digits.
    fn longest(&self) -> usize {
        let date = match self.middle {
            Middle::None => 0,
            Middle::Year => 4 + self.separator.chars().count(),
            Middle::YearMonth => 6 + self.separator.chars().count(),
            Middle::Date => 8 + self.separator.chars().count(),
        };
        self.prefix.chars().count() + self.separator.chars().count() + date + self.padding.max(19)
    }
}

impl Ids for Numbering {
    fn scope(&self, on: NaiveDate) -> String {
        match self.middle {
            Middle::None => String::new(),
            Middle::Year => on.format("%Y").to_string(),
            Middle::YearMonth => on.format("%Y%m").to_string(),
            Middle::Date => on.format("%Y%m%d").to_string(),
        }
    }

    fn id(&self, on: NaiveDate, sequence: i64) -> Name {
        let (prefix, separator, padding) = (&self.prefix, &self.separator, self.padding);
        let middle = match self.middle {
            Middle::None => String::new(),
            _ => format!("{}{separator}", self.scope(on)),
        };
        Name::try_new(format!("{prefix}{separator}{middle}{sequence:0padding$}")).expect("`new` leaves room for any sequence")
    }
}

/// The next id of the org's documents of that kind: the counter of the `ids.scope(on)` is taken atomically
/// (`next = next + 1`, created at 1), in the caller's transaction, so a rolled back document gives its number back.
pub async fn issue<I: Ids + ?Sized>(connection: &mut PgConnection, org_id: Uuid, document: Numbered, on: NaiveDate, ids: &I) -> Result<Name, sqlx::Error> {
    let sequence: i64 = sqlx::query_scalar(
        "INSERT INTO billing_counters (org_id, document, scope, next) VALUES ($1, $2, $3, 2) \
         ON CONFLICT (org_id, document, scope) DO UPDATE SET next = billing_counters.next + 1, updated_at = now() \
         RETURNING next - 1",
    )
    .bind(org_id)
    .bind(document)
    .bind(ids.scope(on))
    .fetch_one(connection)
    .await?;
    Ok(ids.id(on, sequence))
}

/// The ids of an org's invoices and payments, given to the billing router by the app as an axum `Extension`:
/// `.layer(Extension(Formats::new(invoice_ids, payment_ids)))`.
#[derive(Clone)]
pub struct Formats {
    pub invoice: Arc<dyn Ids + Send + Sync>,
    pub payment: Arc<dyn Ids + Send + Sync>,
}

impl Formats {
    pub fn new(invoice: impl Ids + Send + Sync + 'static, payment: impl Ids + Send + Sync + 'static) -> Self {
        Self { invoice: Arc::new(invoice), payment: Arc::new(payment) }
    }
}
