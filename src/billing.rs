//! Plans, prices, add-ons and subscriptions: the megh-go billing tables (created by migration 0006) as row types.
//! Prices are `BIGINT` micros of the row's `currency`; the accessors turn them into `Money`.

use std::collections::BTreeMap;

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::money::{from_micros, iso, Money, MoneyError};
use crate::{JsonText, Table};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "postgres", derive(sqlx::Type), sqlx(type_name = "text", rename_all = "snake_case"))]
pub enum SubscriptionStatus {
    Trialing,
    Active,
    PastDue,
    Canceled,
    Expired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "postgres", derive(sqlx::Type), sqlx(type_name = "text", rename_all = "snake_case"))]
pub enum BillingInterval {
    Monthly,
    Yearly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "postgres", derive(sqlx::Type), sqlx(type_name = "text", rename_all = "snake_case"))]
pub enum AddOnType {
    Recurring,
    OneTime,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Table)]
#[cfg_attr(feature = "postgres", derive(sqlx::FromRow))]
#[table(name = "plans")]
pub struct Plan {
    pub id: Uuid,
    pub sku: String,
    pub name: String,
    pub description: Option<String>,
    #[cfg_attr(feature = "postgres", sqlx(try_from = "Option<String>"))]
    pub features: JsonText<Vec<String>>,
    #[cfg_attr(feature = "postgres", sqlx(try_from = "Option<String>"))]
    pub limits: JsonText<BTreeMap<String, i64>>,
    #[cfg_attr(feature = "postgres", sqlx(try_from = "Option<String>"))]
    pub metadata: JsonText<serde_json::Value>,
    pub active: Option<bool>,
    pub created_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Table)]
#[cfg_attr(feature = "postgres", derive(sqlx::FromRow))]
#[table(name = "plan_prices")]
pub struct PlanPrice {
    pub id: Uuid,
    pub plan_id: Uuid,
    pub sku: String,
    pub interval: BillingInterval,
    pub currency: Option<String>,
    pub base_price: i64,
    pub per_seat_price: Option<i64>,
    pub included_seats: Option<i64>,
    /// Nanoseconds, as megh-go stores a Go `time.Duration`.
    pub trial_duration: Option<i64>,
    #[cfg_attr(feature = "postgres", sqlx(try_from = "Option<String>"))]
    pub metadata: JsonText<serde_json::Value>,
    pub active: Option<bool>,
}

impl PlanPrice {
    pub fn base(&self) -> Result<Money<'static, iso::Currency>, MoneyError> {
        money(self.base_price, &self.currency)
    }

    pub fn per_seat(&self) -> Result<Money<'static, iso::Currency>, MoneyError> {
        money(self.per_seat_price.unwrap_or_default(), &self.currency)
    }

    /// The free trial a new subscription starts with, if any.
    pub fn trial(&self) -> Option<Duration> {
        self.trial_duration.filter(|nanoseconds| *nanoseconds > 0).map(Duration::nanoseconds)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Table)]
#[cfg_attr(feature = "postgres", derive(sqlx::FromRow))]
#[table(name = "add_ons")]
pub struct AddOn {
    pub id: Uuid,
    pub sku: String,
    pub name: String,
    pub description: Option<String>,
    #[serde(rename = "type")]
    #[cfg_attr(feature = "postgres", sqlx(rename = "type"))]
    pub kind: AddOnType,
    pub price: i64,
    pub currency: Option<String>,
    pub interval: Option<BillingInterval>,
    pub unit: Option<String>,
    #[cfg_attr(feature = "postgres", sqlx(try_from = "Option<String>"))]
    pub metadata: JsonText<serde_json::Value>,
    pub active: Option<bool>,
}

impl AddOn {
    pub fn price(&self) -> Result<Money<'static, iso::Currency>, MoneyError> {
        money(self.price, &self.currency)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Table)]
#[cfg_attr(feature = "postgres", derive(sqlx::FromRow))]
#[table(name = "plan_add_ons", keys = ["plan_id", "add_on_id"])]
pub struct PlanAddOn {
    pub plan_id: Uuid,
    pub add_on_id: Uuid,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Table)]
#[cfg_attr(feature = "postgres", derive(sqlx::FromRow))]
#[table(name = "subscriptions")]
pub struct Subscription {
    pub id: Uuid,
    pub organization_id: Uuid,
    pub plan_id: Uuid,
    pub price_id: Uuid,
    pub status: SubscriptionStatus,
    pub seats: Option<i64>,
    pub current_period_start: Option<DateTime<Utc>>,
    pub current_period_end: Option<DateTime<Utc>>,
    pub trial_ends_at: Option<DateTime<Utc>>,
    pub cancel_at_period_end: Option<bool>,
    pub external_id: Option<String>,
    #[cfg_attr(feature = "postgres", sqlx(try_from = "Option<String>"))]
    pub metadata: JsonText<serde_json::Value>,
    pub created_at: Option<DateTime<Utc>>,
    pub updated_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Table)]
#[cfg_attr(feature = "postgres", derive(sqlx::FromRow))]
#[table(name = "subscription_items")]
pub struct SubscriptionItem {
    pub id: Uuid,
    pub subscription_id: Uuid,
    pub add_on_id: Uuid,
    pub quantity: Option<i64>,
    #[cfg_attr(feature = "postgres", sqlx(try_from = "Option<String>"))]
    pub metadata: JsonText<serde_json::Value>,
}

fn money(micros: i64, currency: &Option<String>) -> Result<Money<'static, iso::Currency>, MoneyError> {
    from_micros(micros, currency.as_deref().unwrap_or_default())
}
