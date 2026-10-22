//! Payment: money received for one invoice (a receipt). A refund is kept on the payment, as HubSpot does.

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;
use validator::{Validate, ValidationError};

use super::value::{Micros, PaymentMethod};
use crate::booking::value::{Attributes, Name};
use crate::{Entity, Table};

/// HubSpot's `hs_latest_status` without the refund and dispute states: the refund is the `refunded` amount.
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
#[validate(schema(function = "amounts"))]
pub struct PaymentData {
    pub org_id: Uuid,
    pub invoice_id: Uuid,
    /// Made by the app's `Ids` when the payment is recorded.
    #[schema(value_type = Option<String>)]
    pub number: Option<Name>,
    pub date: NaiveDate,
    pub amount: Micros,
    pub method: PaymentMethod,
    pub status: PaymentStatus,
    /// How much of `amount` was given back.
    #[serde(default)]
    pub refunded: Micros,
    #[serde(default)]
    #[sqlx(json)]
    #[schema(value_type = Object)]
    pub attributes: Attributes,
}

pub type Payment = Entity<Uuid, PaymentData>;

fn amounts(payment: &PaymentData) -> Result<(), ValidationError> {
    let (amount, refunded) = (i64::from(payment.amount), i64::from(payment.refunded));
    (amount > 0 && (0..=amount).contains(&refunded))
        .then_some(())
        .ok_or_else(|| ValidationError::new("the amount must be positive and the refunded part between nothing and all of it"))
}
