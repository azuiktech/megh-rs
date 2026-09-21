# TPD — Billing

**Status:** B1 shipped (money and entities). B2 (`Subscriptions` lifecycle) is designed and pending. Ports megh-go's plans, prices, add-ons and subscriptions; megh-go has no price calculation, invoicing or payment-provider calls, so neither does this.
**Modules:** `src/billing.rs`, `src/money.rs`, `migrations/0006_align_org.sql` (the tables).
**Related:** `TPD-organizations.md` (the billing tables were created there, schema only).

## 1. Delivery

| # | Feature | Status | PR / issue |
|---|---|---|---|
| B1 | `megh::money` and the billing row types with `Money` accessors | `[x]` | #47 / #45 |
| B2 | `Subscriptions`: plans, prices, add-ons, subscribe, change seats, cancel, add-on attach and detach | `[ ]` | #46 |

## 2. Money (`megh::money`)

The price columns are `BIGINT` millionths of a unit of the row's `currency` (`VARCHAR(3)`), as in megh-go: `20000000` with `USD` is $20.00. The scale is one constant; Rust code uses `rusty-money` 0.5 (`Money`, `iso`, decimal amounts, ISO 4217 currencies).

```rust
pub const MICROS_PER_UNIT: i64 = 1_000_000;
pub fn from_micros(micros: i64, currency: &str) -> Result<Money<'static, iso::Currency>, MoneyError>;   // InvalidCurrency for an unknown code
pub fn to_micros(money: &Money<iso::Currency>) -> Result<i64, MoneyError>;                              // Overflow, or PrecisionLoss beyond 6 decimals; never rounds
pub use rusty_money::{iso, Money, MoneyError};
```

Kept as `BIGINT` rather than `NUMERIC` so rows are portable with megh-go and exact on any database (SQLite has no exact decimal type).

## 3. Entities (`megh::billing`)

Plain row types, one per table, with `#[derive(Table)]` (generic insert, upsert, update) and no relations. Nullable columns are `Option`; JSON kept in text columns is `JsonText<T>` (`megh::JsonText`), where NULL and `null` read as the default. Verified against a real megh-go database (1 plan, 1 price, 81 subscriptions).

```rust
pub enum SubscriptionStatus { Trialing, Active, PastDue, Canceled, Expired }   // text column
pub enum BillingInterval { Monthly, Yearly }
pub enum AddOnType { Recurring, OneTime }                                       // column `type`, field `kind`

pub struct Plan { id, sku, name, description, features: JsonText<Vec<String>>, limits: JsonText<BTreeMap<String, i64>>, metadata, active, created_at }
pub struct PlanPrice { id, plan_id, sku, interval, currency, base_price: i64, per_seat_price, included_seats, trial_duration, metadata, active }
pub struct AddOn { id, sku, name, description, kind, price: i64, currency, interval, unit, metadata, active }
pub struct PlanAddOn { plan_id, add_on_id }
pub struct Subscription { id, organization_id, plan_id, price_id, status, seats, current_period_start, current_period_end, trial_ends_at, cancel_at_period_end, external_id, metadata, created_at, updated_at }
pub struct SubscriptionItem { id, subscription_id, add_on_id, quantity, metadata }

impl PlanPrice { pub fn base(&self) -> Result<Money, MoneyError>; pub fn per_seat(&self) -> Result<Money, MoneyError>; pub fn trial(&self) -> Option<Duration> }
impl AddOn { pub fn price(&self) -> Result<Money, MoneyError> }
```

`PlanPrice::trial_duration` is nanoseconds because megh-go stores a Go `time.Duration` (a real row holds `2592000000000000`, 30 days); `trial()` converts it.

## 4. Pending: `Subscriptions` (B2, #46)

```rust
impl Subscriptions {                       // holds the injected pool, like Orgs
    pub fn new(pool: PgPool) -> Self;
    pub async fn create_plan(&self, plan: &Plan) -> Result<Plan, BillingError>;        // also list_plans, plan(id), plan_by_sku
    pub async fn add_price(&self, price: &PlanPrice) -> Result<PlanPrice, BillingError>;
    pub async fn create_add_on(&self, add_on: &AddOn) -> Result<AddOn, BillingError>;  // also attach_plan_add_on
    pub async fn subscription(&self, org_id: Uuid) -> Result<Subscription, BillingError>;
    pub async fn subscribe(&self, org_id: Uuid, price_id: Uuid, seats: i64, metadata: Value) -> Result<Subscription, BillingError>;
    pub async fn change_seats(&self, org_id: Uuid, seats: i64) -> Result<Subscription, BillingError>;
    pub async fn cancel(&self, org_id: Uuid, when: CancelWhen) -> Result<Subscription, BillingError>;   // Now | AtPeriodEnd
    pub async fn attach_add_on(&self, org_id: Uuid, add_on_id: Uuid, quantity: i64, metadata: Value) -> Result<Subscription, BillingError>;
    pub async fn detach_add_on(&self, org_id: Uuid, add_on_id: Uuid) -> Result<Subscription, BillingError>;
}
pub enum BillingError { NotFound, InvalidSeats, Database(sqlx::Error) }   // #[non_exhaustive]
```

Behaviour follows megh-go: `subscribe` replaces the organization's subscription (unique per organization), uses the price's included seats as a minimum, starts a trial (`trialing`, `trial_ends_at`) when the price has a trial, and sets the period end one month or one year ahead; seats are at least 1; an add-on quantity below 1 becomes 1 and re-attaching updates it; `cancel(Now)` sets `canceled`, `cancel(AtPeriodEnd)` sets `cancel_at_period_end`. Unlike megh-go, `subscribe` takes no plan id: the price row names its plan.

## 5. Limits

- Postgres only, like the rest of megh-rs (the `Table` derive builds Postgres SQL).
- No price arithmetic beyond `Money`: no amount-due, proration or invoices (megh-go has none).
