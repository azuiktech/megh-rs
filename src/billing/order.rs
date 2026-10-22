//! Order: what a customer asked to buy (a sales order) or what the org asked a vendor for (a purchase order).
//! A draft order is a quote.

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use super::value::{Currency, ExchangeRate, Micros, Percent, Quantity, TaxMode};
use crate::booking::value::{Address, Attributes, Name};
use crate::{Entity, Table};

/// Odoo `sale.order` and `purchase.order`; Zoho Sales Order and Purchase Order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, sqlx::Type, ToSchema)]
#[sqlx(type_name = "text", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum OrderKind {
    /// Its party is a `booking::client::Client`.
    SalesOrder,
    /// Its party is a `vendor::Vendor`.
    PurchaseOrder,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, sqlx::Type, ToSchema)]
#[sqlx(type_name = "text", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum OrderStatus {
    Draft,
    Open,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, sqlx::FromRow, Table, ToSchema)]
#[table(name = "orders", keys = ["org_id", "id"])]
pub struct OrderData {
    pub org_id: Uuid,
    pub kind: OrderKind,
    /// The client of a sales order, the vendor of a purchase order.
    pub party_id: Uuid,
    /// Taken from the org's series when the order is opened.
    #[schema(value_type = Option<String>)]
    pub number: Option<Name>,
    pub status: OrderStatus,
    pub date: NaiveDate,
    pub expected_date: Option<NaiveDate>,
    #[schema(value_type = Option<String>)]
    pub reference: Option<Name>,
    pub currency: Currency,
    #[serde(default)]
    pub exchange_rate: ExchangeRate,
    pub payment_term_id: Option<Uuid>,
    #[serde(default)]
    pub tax_mode: TaxMode,
    #[serde(default)]
    #[sqlx(json(nullable))]
    pub billing_address: Option<Address>,
    #[serde(default)]
    #[sqlx(json(nullable))]
    pub shipping_address: Option<Address>,
    /// Where the goods are shipped from or received: an org of the org's hierarchy.
    pub location_id: Option<Uuid>,
    #[schema(value_type = Option<String>)]
    pub delivery_method: Option<Name>,
    #[serde(default)]
    pub notes: String,
    #[serde(default)]
    pub terms: String,
    /// Worked out from the lines.
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

pub type Order = Entity<Uuid, OrderData>;

/// A product or a service ordered. A line names at most one of them; a free line names neither.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, sqlx::FromRow, Table, ToSchema)]
#[table(name = "order_lines", keys = ["org_id", "id"])]
pub struct OrderLineData {
    pub org_id: Uuid,
    pub order_id: Uuid,
    pub position: i32,
    pub product_id: Option<Uuid>,
    pub service_id: Option<Uuid>,
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
    pub expected_date: Option<NaiveDate>,
    /// Worked out: quantity times unit price less the discount, then with the taxes.
    #[serde(default)]
    pub sub_total: Micros,
    #[serde(default)]
    pub total: Micros,
}

pub type OrderLine = Entity<Uuid, OrderLineData>;
