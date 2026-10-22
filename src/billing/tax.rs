//! What an org sets up once for its documents: taxes, payment terms and number series.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use validator::Validate;
use uuid::Uuid;

use super::value::{Days, Percent};
use crate::booking::value::{Attributes, Name};
use crate::{Entity, Table};

/// A percentage tax on a line (Odoo `account.tax`, Zoho `Tax`, HubSpot `taxes`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Validate, sqlx::FromRow, Table, ToSchema)]
#[table(name = "billing_taxes", keys = ["org_id", "id"])]
#[schema(example = json!({"name": "Sales tax", "rate": "7.5"}))]
pub struct TaxData {
    pub org_id: Uuid,
    #[schema(value_type = String, example = "Sales tax")]
    pub name: Name,
    pub rate: Percent,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    #[sqlx(json)]
    #[schema(value_type = Object)]
    pub attributes: Attributes,
}

pub type Tax = Entity<Uuid, TaxData>;

/// Days after the invoice date that payment is due (Odoo `account.payment.term`, Zoho `payment_terms`,
/// HubSpot `hs_net_payment_term`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Validate, sqlx::FromRow, Table, ToSchema)]
#[table(name = "billing_payment_terms", keys = ["org_id", "id"])]
#[schema(example = json!({"name": "Net 30", "net_days": 30}))]
pub struct PaymentTermData {
    pub org_id: Uuid,
    #[schema(value_type = String, example = "Net 30")]
    pub name: Name,
    pub net_days: Days,
    #[serde(default)]
    pub description: String,
}

pub type PaymentTerm = Entity<Uuid, PaymentTermData>;

/// A named run of numbers for documents (`INV-0001`): the next number is taken when a document is opened.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Validate, sqlx::FromRow, Table, ToSchema)]
#[table(name = "billing_series", keys = ["org_id", "id"])]
#[schema(example = json!({"name": "Invoices", "prefix": "INV-", "next": 1, "padding": 4}))]
pub struct SeriesData {
    pub org_id: Uuid,
    #[schema(value_type = String, example = "Invoices")]
    pub name: Name,
    #[serde(default)]
    pub prefix: String,
    #[schema(minimum = 1)]
    #[validate(range(min = 1))]
    pub next: i64,
    /// Digits the number is zero-padded to.
    #[serde(default)]
    #[validate(range(min = 0))]
    pub padding: i32,
}

pub type Series = Entity<Uuid, SeriesData>;
