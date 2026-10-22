//! Vendor: who an org buys from. The customer side is `booking::client::Client`.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use super::value::Currency;
use crate::booking::value::{Address, Attributes, Contact, Name};
use crate::{Entity, Table};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, sqlx::FromRow, Table, ToSchema)]
#[table(name = "billing_vendors", keys = ["org_id", "id"])]
#[schema(example = json!({"name": "Acme Supplies", "email": "ap@acme.example", "tax_id": "12-3456789"}))]
pub struct VendorData {
    pub org_id: Uuid,
    #[schema(value_type = String, example = "Acme Supplies")]
    pub name: Name,
    #[serde(default)]
    pub description: String,
    #[serde(flatten)]
    #[sqlx(flatten)]
    pub contact: Contact,
    #[serde(default)]
    #[sqlx(json(nullable))]
    pub address: Option<Address>,
    #[schema(value_type = Option<String>)]
    pub tax_id: Option<Name>,
    pub payment_term_id: Option<Uuid>,
    /// The currency its bills are in unless they say otherwise.
    pub currency: Option<Currency>,
    #[serde(default)]
    #[sqlx(json)]
    #[schema(value_type = Object)]
    pub attributes: Attributes,
}

pub type Vendor = Entity<Uuid, VendorData>;
