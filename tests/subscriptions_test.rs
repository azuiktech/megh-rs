use chrono::{Duration, Months};
use megh::billing::{AddOn, AddOnType, BillingError, BillingInterval, CancelWhen, Plan, PlanPrice, SubscriptionStatus, Subscriptions};
use megh::JsonText;
use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

fn plan(sku: &str, active: bool) -> Plan {
    Plan {
        id: Uuid::now_v7(),
        sku: sku.into(),
        name: sku.into(),
        description: None,
        features: JsonText::default(),
        limits: JsonText::default(),
        metadata: JsonText::default(),
        active: Some(active),
        created_at: None,
    }
}

fn price(plan_id: Uuid, interval: BillingInterval, included_seats: i64, trial_days: i64) -> PlanPrice {
    PlanPrice {
        id: Uuid::now_v7(),
        plan_id,
        sku: Uuid::now_v7().to_string(),
        interval,
        currency: Some("USD".into()),
        base_price: 20_000_000,
        per_seat_price: Some(0),
        included_seats: Some(included_seats),
        trial_duration: Some(Duration::days(trial_days).num_nanoseconds().unwrap()),
        metadata: JsonText::default(),
        active: Some(true),
    }
}

fn add_on(sku: &str) -> AddOn {
    AddOn {
        id: Uuid::now_v7(),
        sku: sku.into(),
        name: sku.into(),
        description: None,
        kind: AddOnType::Recurring,
        price: 1_000_000,
        currency: Some("USD".into()),
        interval: None,
        unit: None,
        metadata: JsonText::default(),
        active: Some(true),
    }
}

async fn price_of(billing: &Subscriptions, interval: BillingInterval, included_seats: i64, trial_days: i64) -> PlanPrice {
    let plan = billing.create_plan(&plan(&Uuid::now_v7().to_string(), true)).await.unwrap();
    billing.add_price(&price(plan.id, interval, included_seats, trial_days)).await.unwrap()
}

#[sqlx::test]
async fn subscribe_starts_a_trial_and_pads_seats_to_the_included_minimum(pool: PgPool) {
    let billing = Subscriptions::new(pool);
    let price = price_of(&billing, BillingInterval::Monthly, 3, 14).await;

    let subscription = billing.subscribe(Uuid::now_v7(), price.id, 1, Value::Null).await.unwrap();

    let start = subscription.current_period_start.unwrap();
    assert_eq!((subscription.status, subscription.seats, subscription.plan_id), (SubscriptionStatus::Trialing, Some(3), price.plan_id));
    assert_eq!(subscription.trial_ends_at, Some(start + Duration::days(14)));
    assert_eq!(subscription.current_period_end, start.checked_add_months(Months::new(1)));
}

#[sqlx::test]
async fn subscribe_without_a_trial_is_active_for_a_year_on_a_yearly_price(pool: PgPool) {
    let billing = Subscriptions::new(pool);
    let price = price_of(&billing, BillingInterval::Yearly, 1, 0).await;

    let subscription = billing.subscribe(Uuid::now_v7(), price.id, 5, serde_json::json!({"source": "test"})).await.unwrap();

    let start = subscription.current_period_start.unwrap();
    assert_eq!((subscription.status, subscription.seats, subscription.trial_ends_at), (SubscriptionStatus::Active, Some(5), None));
    assert_eq!(subscription.current_period_end, start.checked_add_months(Months::new(12)));
    assert_eq!(subscription.metadata.0["source"], "test");
}

#[sqlx::test]
async fn subscribing_again_replaces_the_tenants_subscription(pool: PgPool) {
    let billing = Subscriptions::new(pool.clone());
    let (monthly, yearly) = (price_of(&billing, BillingInterval::Monthly, 1, 0).await, price_of(&billing, BillingInterval::Yearly, 1, 0).await);
    let tenant = Uuid::now_v7();

    let first = billing.subscribe(tenant, monthly.id, 1, Value::Null).await.unwrap();
    let second = billing.subscribe(tenant, yearly.id, 2, Value::Null).await.unwrap();

    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM subscriptions").fetch_one(&pool).await.unwrap();
    assert_eq!((rows, second.id, second.created_at), (1, first.id, first.created_at));
    assert_eq!((second.price_id, second.seats), (yearly.id, Some(2)));
    assert!(matches!(billing.subscribe(tenant, Uuid::now_v7(), 1, Value::Null).await, Err(BillingError::NotFound)));
}

#[sqlx::test]
async fn change_seats_needs_a_subscription_and_at_least_one_seat(pool: PgPool) {
    let billing = Subscriptions::new(pool);
    let price = price_of(&billing, BillingInterval::Monthly, 1, 0).await;
    let tenant = Uuid::now_v7();
    billing.subscribe(tenant, price.id, 1, Value::Null).await.unwrap();

    assert_eq!(billing.change_seats(tenant, 7).await.unwrap().seats, Some(7));
    assert!(matches!(billing.change_seats(tenant, 0).await, Err(BillingError::InvalidSeats)));
    assert!(matches!(billing.change_seats(Uuid::now_v7(), 2).await, Err(BillingError::NotFound)));
}

#[sqlx::test]
async fn cancel_at_period_end_keeps_the_status_and_now_ends_it(pool: PgPool) {
    let billing = Subscriptions::new(pool);
    let price = price_of(&billing, BillingInterval::Monthly, 1, 0).await;
    let tenant = Uuid::now_v7();
    billing.subscribe(tenant, price.id, 1, Value::Null).await.unwrap();

    let later = billing.cancel(tenant, CancelWhen::AtPeriodEnd).await.unwrap();
    assert_eq!((later.status, later.cancel_at_period_end), (SubscriptionStatus::Active, Some(true)));
    let now = billing.cancel(tenant, CancelWhen::Now).await.unwrap();
    assert_eq!((now.status, now.cancel_at_period_end), (SubscriptionStatus::Canceled, Some(false)));
    assert!(matches!(billing.cancel(Uuid::now_v7(), CancelWhen::Now).await, Err(BillingError::NotFound)));
}

#[sqlx::test]
async fn add_ons_attach_update_and_detach(pool: PgPool) {
    let billing = Subscriptions::new(pool.clone());
    let price = price_of(&billing, BillingInterval::Monthly, 1, 0).await;
    let extra = billing.create_add_on(&add_on("storage")).await.unwrap();
    let tenant = Uuid::now_v7();
    let subscription = billing.subscribe(tenant, price.id, 1, Value::Null).await.unwrap();
    let quantity = || async { sqlx::query_scalar::<_, i64>("SELECT quantity FROM subscription_items WHERE subscription_id = $1").bind(subscription.id).fetch_all(&pool).await.unwrap() };

    billing.attach_add_on(tenant, extra.id, 0, Value::Null).await.unwrap();
    assert_eq!(quantity().await, [1]);
    assert_eq!(billing.attach_add_on(tenant, extra.id, 4, Value::Null).await.unwrap().id, subscription.id);
    assert_eq!(quantity().await, [4]);
    billing.detach_add_on(tenant, extra.id).await.unwrap();
    assert!(quantity().await.is_empty());
    assert!(matches!(billing.detach_add_on(tenant, extra.id).await, Err(BillingError::NotFound)));
    assert!(matches!(billing.attach_add_on(Uuid::now_v7(), extra.id, 1, Value::Null).await, Err(BillingError::NotFound)));
}

#[sqlx::test]
async fn plans_are_found_by_id_and_sku_and_listed_when_active(pool: PgPool) {
    let billing = Subscriptions::new(pool.clone());
    let pro = billing.create_plan(&plan("pro", true)).await.unwrap();
    billing.create_plan(&plan("retired", false)).await.unwrap();
    let extra = billing.create_add_on(&add_on("storage")).await.unwrap();

    assert_eq!(billing.plan(pro.id).await.unwrap(), pro);
    assert_eq!(billing.plan_by_sku("pro").await.unwrap(), pro);
    assert_eq!(billing.list_plans().await.unwrap(), [pro.clone()]);
    assert!(matches!(billing.plan_by_sku("missing").await, Err(BillingError::NotFound)));
    assert!(matches!(billing.plan(Uuid::now_v7()).await, Err(BillingError::NotFound)));

    billing.attach_plan_add_on(pro.id, extra.id).await.unwrap();
    billing.attach_plan_add_on(pro.id, extra.id).await.unwrap();
    let links: i64 = sqlx::query_scalar("SELECT count(*) FROM plan_add_ons").fetch_one(&pool).await.unwrap();
    assert_eq!(links, 1);
}

#[sqlx::test]
async fn subscribe_refuses_fewer_than_one_seat(pool: PgPool) {
    let billing = Subscriptions::new(pool);
    let price = price_of(&billing, BillingInterval::Monthly, 0, 0).await;

    for seats in [0, -3] {
        assert!(matches!(billing.subscribe(Uuid::now_v7(), price.id, seats, Value::Null).await, Err(BillingError::InvalidSeats)), "{seats} seats");
    }
}
