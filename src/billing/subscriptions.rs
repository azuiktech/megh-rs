//! The subscription lifecycle over the billing tables, ported from megh-go's `Subscriptions`.

use chrono::{Months, Utc};
use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

use super::{AddOn, BillingInterval, Plan, PlanPrice, Subscription, SubscriptionItem, SubscriptionStatus};
use crate::{JsonText, TableEntity};

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum BillingError {
    #[error("not found")]
    NotFound,
    #[error("seats must be at least 1")]
    InvalidSeats,
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}

/// When a cancellation takes effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CancelWhen {
    Now,
    AtPeriodEnd,
}

/// Plans, prices, add-ons and each organization's subscription; holds the injected pool.
#[derive(Clone)]
pub struct Subscriptions {
    pool: PgPool,
}

impl Subscriptions {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn create_plan(&self, plan: &Plan) -> Result<Plan, BillingError> {
        Ok(plan.insert(&self.pool).await?)
    }

    /// The active plans.
    pub async fn list_plans(&self) -> Result<Vec<Plan>, BillingError> {
        Ok(sqlx::query_as("SELECT * FROM plans WHERE active = true ORDER BY sku").fetch_all(&self.pool).await?)
    }

    pub async fn plan(&self, id: Uuid) -> Result<Plan, BillingError> {
        found(sqlx::query_as("SELECT * FROM plans WHERE id = $1").bind(id).fetch_optional(&self.pool).await?)
    }

    pub async fn plan_by_sku(&self, sku: &str) -> Result<Plan, BillingError> {
        found(sqlx::query_as("SELECT * FROM plans WHERE sku = $1").bind(sku).fetch_optional(&self.pool).await?)
    }

    pub async fn add_price(&self, price: &PlanPrice) -> Result<PlanPrice, BillingError> {
        Ok(price.insert(&self.pool).await?)
    }

    pub async fn create_add_on(&self, add_on: &AddOn) -> Result<AddOn, BillingError> {
        Ok(add_on.insert(&self.pool).await?)
    }

    pub async fn attach_plan_add_on(&self, plan_id: Uuid, add_on_id: Uuid) -> Result<(), BillingError> {
        sqlx::query("INSERT INTO plan_add_ons (plan_id, add_on_id) VALUES ($1, $2) ON CONFLICT DO NOTHING")
            .bind(plan_id)
            .bind(add_on_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn subscription(&self, org_id: Uuid) -> Result<Subscription, BillingError> {
        found(self.find_subscription(org_id).await?)
    }

    /// Replaces the organization's subscription, keeping its id. Seats are at least the price's included seats.
    pub async fn subscribe(&self, org_id: Uuid, price_id: Uuid, seats: i64, metadata: Value) -> Result<Subscription, BillingError> {
        let price: PlanPrice = found(sqlx::query_as("SELECT * FROM plan_prices WHERE id = $1").bind(price_id).fetch_optional(&self.pool).await?)?;
        let fresh = new_subscription(org_id, &price, seats, metadata);
        let subscription = match self.find_subscription(org_id).await? {
            Some(existing) => Subscription { id: existing.id, created_at: existing.created_at, ..fresh },
            None => fresh,
        };
        Ok(subscription.upsert(&self.pool).await?)
    }

    pub async fn change_seats(&self, org_id: Uuid, seats: i64) -> Result<Subscription, BillingError> {
        (seats >= 1).then_some(()).ok_or(BillingError::InvalidSeats)?;
        found(
            sqlx::query_as("UPDATE subscriptions SET seats = $2, updated_at = NOW() WHERE organization_id = $1 RETURNING *")
                .bind(org_id)
                .bind(seats)
                .fetch_optional(&self.pool)
                .await?,
        )
    }

    pub async fn cancel(&self, org_id: Uuid, when: CancelWhen) -> Result<Subscription, BillingError> {
        let (status, at_period_end) = match when {
            CancelWhen::Now => (Some(SubscriptionStatus::Canceled), false),
            CancelWhen::AtPeriodEnd => (None, true),
        };
        found(
            sqlx::query_as("UPDATE subscriptions SET status = COALESCE($2, status), cancel_at_period_end = $3, updated_at = NOW() WHERE organization_id = $1 RETURNING *")
                .bind(org_id)
                .bind(status)
                .bind(at_period_end)
                .fetch_optional(&self.pool)
                .await?,
        )
    }

    /// Attaches the add-on, or updates its quantity and metadata when already attached. A quantity below 1 is 1.
    pub async fn attach_add_on(&self, org_id: Uuid, add_on_id: Uuid, quantity: i64, metadata: Value) -> Result<Subscription, BillingError> {
        let subscription = self.subscription(org_id).await?;
        let quantity = quantity.max(1);
        let text = (!metadata.is_null()).then(|| metadata.to_string());
        let updated = sqlx::query("UPDATE subscription_items SET quantity = $3, metadata = $4 WHERE subscription_id = $1 AND add_on_id = $2")
            .bind(subscription.id)
            .bind(add_on_id)
            .bind(quantity)
            .bind(text)
            .execute(&self.pool)
            .await?;
        if updated.rows_affected() == 0 {
            let item = SubscriptionItem { id: Uuid::new_v4(), subscription_id: subscription.id, add_on_id, quantity: Some(quantity), metadata: JsonText(metadata) };
            item.insert(&self.pool).await?;
        }
        Ok(subscription)
    }

    pub async fn detach_add_on(&self, org_id: Uuid, add_on_id: Uuid) -> Result<Subscription, BillingError> {
        let subscription = self.subscription(org_id).await?;
        let deleted = sqlx::query("DELETE FROM subscription_items WHERE subscription_id = $1 AND add_on_id = $2")
            .bind(subscription.id)
            .bind(add_on_id)
            .execute(&self.pool)
            .await?;
        (deleted.rows_affected() > 0).then_some(subscription).ok_or(BillingError::NotFound)
    }

    async fn find_subscription(&self, org_id: Uuid) -> Result<Option<Subscription>, sqlx::Error> {
        sqlx::query_as("SELECT * FROM subscriptions WHERE organization_id = $1").bind(org_id).fetch_optional(&self.pool).await
    }
}

fn found<T>(row: Option<T>) -> Result<T, BillingError> {
    row.ok_or(BillingError::NotFound)
}

fn new_subscription(org_id: Uuid, price: &PlanPrice, seats: i64, metadata: Value) -> Subscription {
    let now = Utc::now();
    let trial = price.trial();
    let months = match price.interval {
        BillingInterval::Monthly => 1,
        BillingInterval::Yearly => 12,
    };
    Subscription {
        id: Uuid::new_v4(),
        organization_id: org_id,
        plan_id: price.plan_id,
        price_id: price.id,
        status: trial.map_or(SubscriptionStatus::Active, |_| SubscriptionStatus::Trialing),
        seats: Some(seats.max(price.included_seats.unwrap_or(1))),
        current_period_start: Some(now),
        current_period_end: now.checked_add_months(Months::new(months)),
        trial_ends_at: trial.map(|trial| now + trial),
        cancel_at_period_end: Some(false),
        external_id: None,
        metadata: JsonText(metadata),
        created_at: Some(now),
        updated_at: Some(now),
    }
}
