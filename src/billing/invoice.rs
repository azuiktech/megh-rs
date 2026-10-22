//! Invoice: a request for payment. One entity for all four documents, told apart by `kind` as Zoho names them
//! (Odoo has them as the `move_type` of one model). An open invoice is not changed: a mistake is corrected by
//! a credit note or a vendor credit.
//!
//! Whether it is paid, partly paid or overdue is not stored: it follows from its `Application`s and `due_date`.

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use validator::{Validate, ValidationError};
use uuid::Uuid;

use super::party::Party;
use super::value::{Currency, ExchangeRate, Micros, Percent, Quantity, TaxMode};
use crate::booking::value::{Address, Attributes, Name};
use crate::{Entity, Table};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, sqlx::Type, ToSchema)]
#[sqlx(type_name = "text", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum InvoiceKind {
    /// Billed to a client.
    Invoice,
    /// Reduces what a client owes.
    CreditNote,
    /// Billed by a vendor.
    Bill,
    /// Reduces what is owed to a vendor.
    VendorCredit,
}

impl InvoiceKind {
    /// What a credit note or vendor credit credits.
    pub fn credits(self) -> Option<InvoiceKind> {
        match self {
            Self::CreditNote => Some(Self::Invoice),
            Self::VendorCredit => Some(Self::Bill),
            Self::Invoice | Self::Bill => None,
        }
    }

    pub fn party(self) -> Party {
        match self {
            Self::Invoice | Self::CreditNote => Party::Client,
            Self::Bill | Self::VendorCredit => Party::Vendor,
        }
    }
}

/// HubSpot's `hs_invoice_status` without `paid`, which is derived.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, sqlx::Type, ToSchema)]
#[sqlx(type_name = "text", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum InvoiceStatus {
    Draft,
    Open,
    Void,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Validate, sqlx::FromRow, Table, ToSchema)]
#[table(name = "billing_invoices", keys = ["org_id", "id"])]
#[validate(schema(function = "credits_only_a_credit"))]
pub struct InvoiceData {
    pub org_id: Uuid,
    pub kind: InvoiceKind,
    /// The client of an invoice or credit note, the vendor of a bill or vendor credit.
    pub party_id: Uuid,
    /// Taken from the org's series when the invoice is opened; a vendor's own number goes in `reference`.
    #[schema(value_type = Option<String>)]
    pub number: Option<Name>,
    pub status: InvoiceStatus,
    pub date: NaiveDate,
    pub due_date: NaiveDate,
    pub payment_term_id: Option<Uuid>,
    #[schema(value_type = Option<String>)]
    pub reference: Option<Name>,
    pub currency: Currency,
    #[serde(default)]
    pub exchange_rate: ExchangeRate,
    #[serde(default)]
    pub tax_mode: TaxMode,
    /// Copies, so that an open invoice keeps the addresses it was issued with.
    #[serde(default)]
    #[sqlx(json(nullable))]
    pub billing_address: Option<Address>,
    #[serde(default)]
    #[sqlx(json(nullable))]
    pub shipping_address: Option<Address>,
    pub order_id: Option<Uuid>,
    /// The invoice or bill a credit note or vendor credit reduces.
    pub credited_invoice_id: Option<Uuid>,
    pub location_id: Option<Uuid>,
    #[serde(default)]
    pub notes: String,
    #[serde(default)]
    pub terms: String,
    /// Worked out from the lines and fixed when the invoice is opened.
    #[serde(default)]
    pub sub_total: Micros,
    #[serde(default)]
    pub tax_total: Micros,
    #[serde(default)]
    pub total: Micros,
    #[serde(default)]
    #[sqlx(json)]
    #[schema(value_type = Object)]
    pub attributes: Attributes,
}

pub type Invoice = Entity<Uuid, InvoiceData>;

/// What is billed: a product, a service (with the appointment it was for), or neither for a free line.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Validate, sqlx::FromRow, Table, ToSchema)]
#[table(name = "billing_invoice_lines", keys = ["org_id", "id"])]
#[validate(schema(function = "names_one_target"))]
pub struct InvoiceLineData {
    pub org_id: Uuid,
    pub invoice_id: Uuid,
    pub position: i32,
    pub product_id: Option<Uuid>,
    pub service_id: Option<Uuid>,
    pub appointment_id: Option<Uuid>,
    pub order_line_id: Option<Uuid>,
    #[schema(value_type = String, example = "Tooth cleaning")]
    pub description: Name,
    pub quantity: Quantity,
    #[schema(value_type = Option<String>, example = "hour")]
    pub unit: Option<Name>,
    pub unit_price: Micros,
    #[serde(default)]
    pub discount: Percent,
    #[serde(default)]
    pub tax_ids: Vec<Uuid>,
    #[serde(default)]
    pub sub_total: Micros,
    #[serde(default)]
    pub total: Micros,
}

pub type InvoiceLine = Entity<Uuid, InvoiceLineData>;

fn credits_only_a_credit(invoice: &InvoiceData) -> Result<(), ValidationError> {
    (invoice.credited_invoice_id.is_none() || invoice.kind.credits().is_some())
        .then_some(())
        .ok_or_else(|| ValidationError::new("only a credit note or a vendor credit credits an invoice"))
}

fn names_one_target(line: &InvoiceLineData) -> Result<(), ValidationError> {
    (line.product_id.is_none() || line.service_id.is_none())
        .then_some(())
        .ok_or_else(|| ValidationError::new("a line names a product or a service, not both"))
}
