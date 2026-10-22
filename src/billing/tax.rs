//! Tax: what an org sets up once for its invoices. A percentage for now; a fixed amount (a voucher applied to a
//! payment) comes later.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use validator::Validate;
use uuid::Uuid;

use super::value::Percent;
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
