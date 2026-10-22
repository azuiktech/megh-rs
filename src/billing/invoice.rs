//! Invoice: a request for payment from a customer, which is an org. A draft is edited freely; `:open` gives it its
//! number and fixes its totals, and an open invoice is not changed. Whether it is paid or overdue follows from
//! `amount_paid`, `balance_due` and `due_date`.

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;
use validator::{Validate, ValidationError};

use super::draft::Line;
use super::value::{Currency, Micros, Percent, Quantity};
use crate::booking::value::{Address, Attributes, Name};
use crate::{Entity, Table};

/// HubSpot's `hs_invoice_status` without `paid`, which follows from the amounts.
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
pub struct InvoiceData {
    /// The seller.
    pub org_id: Uuid,
    /// The customer: an org.
    pub customer_id: Uuid,
    /// Made by the app's `Ids` when the invoice is opened.
    #[schema(value_type = Option<String>)]
    pub number: Option<Name>,
    pub status: InvoiceStatus,
    pub date: NaiveDate,
    pub due_date: NaiveDate,
    pub currency: Currency,
    /// The customer's address, copied when the invoice is opened.
    #[serde(default)]
    #[sqlx(json(nullable))]
    pub bill_to: Option<Address>,
    #[serde(default)]
    pub comments: String,
    /// Worked out from the lines and fixed when the invoice is opened (HubSpot stores the same amounts).
    #[serde(default)]
    pub sub_total: Micros,
    #[serde(default)]
    pub tax_total: Micros,
    #[serde(default)]
    pub total: Micros,
    #[serde(default)]
    pub amount_paid: Micros,
    #[serde(default)]
    pub balance_due: Micros,
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
    #[schema(value_type = String, example = "Tooth cleaning")]
    pub description: Name,
    pub quantity: Quantity,
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

impl Line for InvoiceLineData {
    type Parent = InvoiceData;
    const PARENT_ID: &'static str = "invoice_id";
}

fn names_one_target(line: &InvoiceLineData) -> Result<(), ValidationError> {
    (line.product_id.is_none() || line.service_id.is_none())
        .then_some(())
        .ok_or_else(|| ValidationError::new("a line names a product or a service, not both"))
}
