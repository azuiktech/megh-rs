//! Payment: money that moved, in or out, and what it was applied to. The payer descriptor on a booking
//! appointment is `booking::appointment::Payment`; this is the money itself.

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use super::value::{Currency, ExchangeRate, Micros, PaymentMethod};
use crate::booking::value::{Attributes, Name};
use crate::{Entity, Table};

/// Odoo `payment_type`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, sqlx::Type, ToSchema)]
#[sqlx(type_name = "text", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum PaymentKind {
    /// Received from a client.
    Inbound,
    /// Paid to a vendor.
    Outbound,
}

/// HubSpot's `hs_latest_status` without the refund and dispute states: a refund is a payment the other way.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, sqlx::Type, ToSchema)]
#[sqlx(type_name = "text", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum PaymentStatus {
    Processing,
    Succeeded,
    Failed,
    Void,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, sqlx::FromRow, Table, ToSchema)]
#[table(name = "billing_payments", keys = ["org_id", "id"])]
pub struct PaymentData {
    pub org_id: Uuid,
    pub kind: PaymentKind,
    /// The client who paid, or the vendor who was paid.
    pub party_id: Uuid,
    /// Taken from the org's series.
    #[schema(value_type = Option<String>)]
    pub number: Option<Name>,
    pub date: NaiveDate,
    pub amount: Micros,
    pub currency: Currency,
    #[serde(default)]
    pub exchange_rate: ExchangeRate,
    pub method: PaymentMethod,
    pub status: PaymentStatus,
    #[schema(value_type = Option<String>)]
    pub reference: Option<Name>,
    #[serde(default)]
    pub description: String,
    /// What the bank or processor charged (Zoho `bank_charges`, HubSpot `hs_fees_amount`).
    #[serde(default)]
    pub fees: Micros,
    pub location_id: Option<Uuid>,
    #[serde(default)]
    #[sqlx(json)]
    #[schema(value_type = Object)]
    pub attributes: Attributes,
}

pub type Payment = Entity<Uuid, PaymentData>;

/// Part of a payment, or of a credit note or vendor credit, set against an invoice or bill (Zoho
/// `invoices[].amount_applied`, Odoo's reconciliation). Exactly one of `payment_id` and `credit_id` is set.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, sqlx::FromRow, Table, ToSchema)]
#[table(name = "billing_applications", keys = ["org_id", "id"])]
pub struct ApplicationData {
    pub org_id: Uuid,
    pub invoice_id: Uuid,
    pub payment_id: Option<Uuid>,
    /// An invoice of kind credit note or vendor credit.
    pub credit_id: Option<Uuid>,
    pub amount: Micros,
}

pub type Application = Entity<Uuid, ApplicationData>;
