//! Specification (issue #238): the app's `Lifecycle` function runs after each tenant, member or subscription change
//! megh commits, with megh's pool and the change. A failing function's error is the call's; the change stays committed.

use std::future::Future;
use std::pin::Pin;

use chrono::Duration;
use megh::billing::{AddOn, AddOnType, BillingError, BillingInterval, CancelWhen, Plan, PlanPrice, Subscriptions};
use megh::{Change, JsonText, LifecycleError, TenantError, Tenants, UserRepo};
use sqlx::PgPool;
use uuid::Uuid;

type Done = Pin<Box<dyn Future<Output = Result<(), LifecycleError>> + Send>>;

/// The function under test: records which change it was given, and for which tenant, in `seen`.
fn record(pool: PgPool, change: Change) -> Done {
    let (kind, tenant_id) = match change {
        Change::TenantCreated(tenant) => ("tenant created", tenant.id),
        Change::MemberAdded(member) => ("member added", member.tenant_id),
        Change::Subscribed(subscription) => ("subscribed", subscription.tenant_id),
        Change::SubscriptionChanged(subscription) => ("subscription changed", subscription.tenant_id),
        Change::SubscriptionCancelled(subscription) => ("subscription cancelled", subscription.tenant_id),
        _ => ("other", Uuid::nil()),
    };
    Box::pin(async move {
        sqlx::query("INSERT INTO seen (kind, tenant_id) VALUES ($1, $2)").bind(kind).bind(tenant_id).execute(&pool).await?;
        Ok(())
    })
}

fn refuse(_: PgPool, _: Change) -> Done {
    Box::pin(async { Err("refused".into()) })
}

async fn seen_table(pool: &PgPool) {
    sqlx::query("CREATE TABLE seen (at SERIAL, kind TEXT NOT NULL, tenant_id UUID NOT NULL)").execute(pool).await.unwrap();
}

async fn seen(pool: &PgPool) -> Vec<(String, Uuid)> {
    sqlx::query_as("SELECT kind, tenant_id FROM seen ORDER BY at").fetch_all(pool).await.unwrap()
}

async fn user(pool: &PgPool) -> Uuid {
    let email = format!("{}@example.com", Uuid::now_v7());
    sqlx::query_scalar("INSERT INTO users (provider, account_id, email) VALUES ('test', $1, $1) RETURNING id").bind(email).fetch_one(pool).await.unwrap()
}

async fn tenant(pool: &PgPool) -> Uuid {
    sqlx::query_scalar("INSERT INTO tenants (name) VALUES ('Acme') RETURNING id").fetch_one(pool).await.unwrap()
}

async fn price(billing: &Subscriptions) -> PlanPrice {
    let sku = Uuid::now_v7().to_string();
    let plan = Plan { id: Uuid::now_v7(), sku: sku.clone(), name: sku, description: None, features: JsonText::default(), limits: JsonText::default(), metadata: JsonText::default(), active: Some(true), created_at: None };
    let plan = billing.create_plan(&plan).await.unwrap();
    let price = PlanPrice {
        id: Uuid::now_v7(),
        plan_id: plan.id,
        sku: Uuid::now_v7().to_string(),
        interval: BillingInterval::Monthly,
        currency: Some("USD".into()),
        base_price: 20_000_000,
        per_seat_price: Some(0),
        included_seats: Some(1),
        trial_duration: Some(Duration::days(0).num_nanoseconds().unwrap()),
        metadata: JsonText::default(),
        active: Some(true),
    };
    billing.add_price(&price).await.unwrap()
}

async fn add_on(billing: &Subscriptions) -> AddOn {
    let sku = Uuid::now_v7().to_string();
    let add_on = AddOn { id: Uuid::now_v7(), sku: sku.clone(), name: sku, description: None, kind: AddOnType::Recurring, price: 1_000_000, currency: Some("USD".into()), interval: None, unit: None, metadata: JsonText::default(), active: Some(true) };
    billing.create_add_on(&add_on).await.unwrap()
}

#[sqlx::test]
async fn creating_a_tenant_runs_the_function_with_the_created_tenant(pool: PgPool) {
    seen_table(&pool).await;
    let owner = user(&pool).await;

    let tenant = Tenants::new(pool.clone()).with_lifecycle(Some(record)).create("Acme", owner).await.unwrap();

    assert_eq!(seen(&pool).await, [("tenant created".to_string(), tenant.id)]);
}

#[sqlx::test]
async fn a_failing_function_fails_the_create_and_the_tenant_stays(pool: PgPool) {
    let owner = user(&pool).await;

    let result = Tenants::new(pool.clone()).with_lifecycle(Some(refuse)).create("Acme", owner).await;

    assert!(matches!(result, Err(TenantError::Lifecycle(_))), "{result:?}");
    let tenants: i64 = sqlx::query_scalar("SELECT count(*) FROM tenants WHERE created_by = $1").bind(owner).fetch_one(&pool).await.unwrap();
    assert_eq!(tenants, 1);
}

#[sqlx::test]
async fn without_a_function_a_change_runs_nothing(pool: PgPool) {
    let owner = user(&pool).await;

    assert!(Tenants::new(pool.clone()).create("Acme", owner).await.is_ok());
}

#[sqlx::test]
async fn registering_a_machine_runs_the_function_with_its_membership(pool: PgPool) {
    seen_table(&pool).await;
    let tenant = tenant(&pool).await;

    UserRepo::new(&pool).with_lifecycle(Some(record)).register_machine(tenant, "ci", vec![]).await.unwrap();

    assert_eq!(seen(&pool).await, [("member added".to_string(), tenant)]);
}

#[sqlx::test]
async fn subscribing_changing_seats_and_cancelling_run_the_function_in_turn(pool: PgPool) {
    seen_table(&pool).await;
    let tenant = tenant(&pool).await;
    let billing = Subscriptions::new(pool.clone()).with_lifecycle(Some(record));
    let price = price(&billing).await;

    billing.subscribe(tenant, price.id, 2, serde_json::Value::Null).await.unwrap();
    billing.change_seats(tenant, 3).await.unwrap();
    billing.cancel(tenant, CancelWhen::Now).await.unwrap();

    let kinds: Vec<String> = seen(&pool).await.into_iter().map(|(kind, at)| { assert_eq!(at, tenant); kind }).collect();
    assert_eq!(kinds, ["subscribed", "subscription changed", "subscription cancelled"]);
}

#[sqlx::test]
async fn attaching_and_detaching_an_add_on_change_the_subscription(pool: PgPool) {
    seen_table(&pool).await;
    let tenant = tenant(&pool).await;
    let quiet = Subscriptions::new(pool.clone());
    let (price, add_on) = (price(&quiet).await, add_on(&quiet).await);
    quiet.subscribe(tenant, price.id, 1, serde_json::Value::Null).await.unwrap();
    let billing = quiet.with_lifecycle(Some(record));

    billing.attach_add_on(tenant, add_on.id, 1, serde_json::Value::Null).await.unwrap();
    billing.detach_add_on(tenant, add_on.id).await.unwrap();

    assert_eq!(seen(&pool).await, [("subscription changed".to_string(), tenant), ("subscription changed".to_string(), tenant)]);
}

#[sqlx::test]
async fn a_failing_function_fails_the_subscription_change_and_the_change_stays(pool: PgPool) {
    let tenant = tenant(&pool).await;
    let quiet = Subscriptions::new(pool.clone());
    let price = price(&quiet).await;
    quiet.subscribe(tenant, price.id, 1, serde_json::Value::Null).await.unwrap();

    let result = quiet.with_lifecycle(Some(refuse)).change_seats(tenant, 4).await;

    assert!(matches!(result, Err(BillingError::Lifecycle(_))), "{result:?}");
    assert_eq!(Subscriptions::new(pool).subscription(tenant).await.unwrap().seats, Some(4));
}
