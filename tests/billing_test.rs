use chrono::{Duration, Utc};
use megh::billing::{AddOn, AddOnType, BillingInterval, Plan, PlanPrice, Subscription, SubscriptionStatus};
use megh::money::{from_micros, iso, to_micros, Money, MoneyError};
use megh::{JsonText, TableEntity};
use rust_decimal::Decimal;
use sqlx::PgPool;
use uuid::Uuid;

const KYRIOS_PLAN: &str = "INSERT INTO plans (id, sku, name, description, features, active, created_at) VALUES
    ('e1946bcd-991a-4e8d-92cc-970524c6388b', 'kyrios-standard', 'Kyrios Standard', 'Full access', '[\"interactive_episodes\",\"ai_tutoring\"]', true, NOW())";
const KYRIOS_PRICE: &str = "INSERT INTO plan_prices (id, plan_id, sku, interval, currency, base_price, per_seat_price, included_seats, trial_duration, active) VALUES
    ('cd97d14e-cc1b-4371-ab27-5082d98bea99', 'e1946bcd-991a-4e8d-92cc-970524c6388b', 'kyrios-standard-monthly', 'monthly', 'USD', 20000000, 0, 1, 2592000000000000, true)";

#[test]
fn micros_convert_to_money_and_back() {
    let price = from_micros(20_000_000, "USD").unwrap();
    assert_eq!(*price.amount(), Decimal::new(20, 0));
    assert_eq!(to_micros(&price).unwrap(), 20_000_000);
    assert_eq!(to_micros(&from_micros(1, "USD").unwrap()).unwrap(), 1);
    assert_eq!(to_micros(&from_micros(-5_000_000, "JPY").unwrap()).unwrap(), -5_000_000);
}

#[test]
fn conversion_reports_what_cannot_be_represented() {
    assert!(matches!(from_micros(1, "ZZZ"), Err(MoneyError::InvalidCurrency)));
    assert!(matches!(to_micros(&Money::from_decimal(Decimal::new(1, 7), iso::USD)), Err(MoneyError::PrecisionLoss)));
    assert!(matches!(to_micros(&Money::from_decimal(Decimal::MAX, iso::USD)), Err(MoneyError::Overflow)));
}

#[sqlx::test]
async fn entities_read_rows_written_by_megh_go(pool: PgPool) {
    sqlx::raw_sql(KYRIOS_PLAN).execute(&pool).await.unwrap();
    sqlx::raw_sql(KYRIOS_PRICE).execute(&pool).await.unwrap();

    let plan: Plan = sqlx::query_as("SELECT * FROM plans").fetch_one(&pool).await.unwrap();
    let price: PlanPrice = sqlx::query_as("SELECT * FROM plan_prices").fetch_one(&pool).await.unwrap();

    assert_eq!(plan.features.0, ["interactive_episodes", "ai_tutoring"]);
    assert!(plan.limits.0.is_empty() && plan.metadata.0.is_null());
    assert_eq!(price.interval, BillingInterval::Monthly);
    assert_eq!(price.base().unwrap(), from_micros(20_000_000, "USD").unwrap());
    assert_eq!(price.per_seat().unwrap(), from_micros(0, "USD").unwrap());
    assert_eq!(price.trial(), Some(Duration::days(30)));
}

#[sqlx::test]
async fn entities_write_the_columns_megh_go_reads(pool: PgPool) {
    let plan = Plan {
        id: Uuid::new_v4(),
        sku: "pro".into(),
        name: "Pro".into(),
        description: None,
        features: JsonText(vec!["a".into()]),
        limits: JsonText([("seats".to_string(), 5)].into()),
        metadata: JsonText(serde_json::json!({"tier": 2})),
        active: Some(true),
        created_at: Some(Utc::now()),
    };
    plan.insert(&pool).await.unwrap();
    let add_on = AddOn {
        id: Uuid::new_v4(),
        sku: "storage".into(),
        name: "Storage".into(),
        description: None,
        kind: AddOnType::Recurring,
        price: 1_500_000,
        currency: Some("EUR".into()),
        interval: Some(BillingInterval::Yearly),
        unit: Some("gb".into()),
        metadata: JsonText(serde_json::Value::Null),
        active: Some(true),
    };
    add_on.insert(&pool).await.unwrap();

    let (features, limits): (String, String) = sqlx::query_as("SELECT features, limits FROM plans").fetch_one(&pool).await.unwrap();
    let (kind, price): (String, i64) = sqlx::query_as("SELECT type, price FROM add_ons").fetch_one(&pool).await.unwrap();
    assert_eq!(serde_json::from_str::<serde_json::Value>(&features).unwrap(), serde_json::json!(["a"]));
    assert_eq!(serde_json::from_str::<serde_json::Value>(&limits).unwrap(), serde_json::json!({"seats": 5}));
    assert_eq!((kind.as_str(), price), ("recurring", 1_500_000));
    assert_eq!(add_on.price().unwrap(), from_micros(1_500_000, "EUR").unwrap());
    assert_eq!(add_on.update(&pool).await.unwrap().kind, AddOnType::Recurring);
}

#[sqlx::test]
async fn subscription_status_reads_megh_go_text(pool: PgPool) {
    sqlx::query("INSERT INTO subscriptions (id, organization_id, plan_id, price_id, status, seats) VALUES ($1, $2, $3, $4, 'past_due', 3)")
        .bind(Uuid::new_v4())
        .bind(Uuid::new_v4())
        .bind(Uuid::new_v4())
        .bind(Uuid::new_v4())
        .execute(&pool)
        .await
        .unwrap();

    let subscription: Subscription = sqlx::query_as("SELECT * FROM subscriptions").fetch_one(&pool).await.unwrap();

    assert_eq!((subscription.status, subscription.seats), (SubscriptionStatus::PastDue, Some(3)));
}
