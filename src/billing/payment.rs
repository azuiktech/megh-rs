//! Payment: money that moved, in or out, and what it was applied to. The payer descriptor on a booking
//! appointment is `booking::appointment::Payment`; this is the money itself.

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use validator::{Validate, ValidationError};
use uuid::Uuid;

use super::party::Party;
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

impl PaymentKind {
    pub fn party(self) -> Party {
        match self {
            Self::Inbound => Party::Client,
            Self::Outbound => Party::Vendor,
        }
    }
}

/// HubSpot's `hs_latest_status` without the refund and dispute states (refunds are designed in B6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, sqlx::Type, ToSchema)]
#[sqlx(type_name = "text", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum PaymentStatus {
    Processing,
    Succeeded,
    Failed,
    Void,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Validate, sqlx::FromRow, Table, ToSchema)]
#[table(name = "billing_payments", keys = ["org_id", "id"])]
#[validate(schema(function = "amount_and_fees"))]
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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Validate, sqlx::FromRow, Table, ToSchema)]
#[table(name = "billing_applications", keys = ["org_id", "id"])]
#[validate(schema(function = "one_source_and_amount"))]
pub struct ApplicationData {
    pub org_id: Uuid,
    pub invoice_id: Uuid,
    pub payment_id: Option<Uuid>,
    /// An invoice of kind credit note or vendor credit.
    pub credit_id: Option<Uuid>,
    pub amount: Micros,
}

pub type Application = Entity<Uuid, ApplicationData>;

fn amount_and_fees(payment: &PaymentData) -> Result<(), ValidationError> {
    (i64::from(payment.amount) > 0 && i64::from(payment.fees) >= 0)
        .then_some(())
        .ok_or_else(|| ValidationError::new("the amount must be positive and the fees not negative"))
}

fn one_source_and_amount(application: &ApplicationData) -> Result<(), ValidationError> {
    (application.payment_id.is_some() != application.credit_id.is_some() && i64::from(application.amount) > 0)
        .then_some(())
        .ok_or_else(|| ValidationError::new("an application is of a payment or a credit, and its amount is positive"))
}
