# TPD — Billing

**Status:** B1 (money and entities) and B2 (`Subscriptions` lifecycle) shipped. Ports megh-go's plans, prices, add-ons and subscriptions; megh-go has no price calculation, invoicing or payment-provider calls, so neither does this.
**Modules:** `src/billing/mod.rs`, `src/billing/subscriptions.rs`, `src/money.rs`, `migrations/0006_align_org.sql` (the tables).
**Related:** `TPD-organizations.md` (the billing tables were created there, schema only).

## 1. Delivery

| # | Feature | Status | PR / issue |
|---|---|---|---|
| B1 | `megh::money` and the billing row types with `Money` accessors | `[x]` | #47 / #45 |
| B2 | `Subscriptions`: plans, prices, add-ons, subscribe, change seats, cancel, add-on attach and detach | `[x]` | #48 / #46 |
| B3 to B5e | A first, larger document model (vendors, orders, credit notes, applications, payment terms, series): types, migration 0017 and CRUD routes, with the shared OpenAPI modifiers in `megh::auth::openapi`. Cut to the simple model of section 6 in S1; the history is in `CHANGELOG.md` | `[x]` | #172 to #184 |
| S1 | Simplify: tax, invoice, invoice line, payment only; migration `0018_billing_simplified.sql` replaces 0017 (the old shape is dropped once when found); helpers use `T::TABLE_NAME`; routes and tests rewritten | `[~]` | #186 / #185 |
| S2 | Numbering: the `Ids` function, a counter row per org, kind and scope, taken atomically | `[ ]` | — |
| S3 | `:open` (number, totals, `bill_to`, lock) and `:void`; tax rounding with `rusty_money::Money::round` | `[ ]` | — |
| S4 | Payments against the invoice balance: `amount_paid`, `balance_due`, refunds on the payment | `[ ]` | — |
| S5 | `parent_id` on `organizations`; a customer org must have the seller as its parent; creating a `Client` creates its org | `[ ]` | — |
| S6 | Product (fields from Odoo and Zoho), then inventory: stock levels and stock moves | `[ ]` | — |

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

## 4. `Subscriptions` (`billing::subscriptions`, feature `postgres`; #48 / #46)

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

`list_plans` returns the active plans only (`active = true`). megh-go preloads each plan's prices and add-ons and a subscription's add-ons through GORM relations; here every call returns the plain row, and there is no accessor for a subscription's add-ons yet (attach and detach return the `Subscription`).

Behaviour follows megh-go: `subscribe` replaces the organization's subscription (unique per organization), uses the price's included seats as a minimum, starts a trial (`trialing`, `trial_ends_at`) when the price has a trial, and sets the period end one month or one year ahead; seats are at least 1; an add-on quantity below 1 becomes 1 and re-attaching updates it; `cancel(Now)` sets `canceled`, `cancel(AtPeriodEnd)` sets `cancel_at_period_end`. `subscribe` reads and then upserts (megh-go used a transaction), so two simultaneous first subscriptions of one organization end with a unique-index error for one of them. Unlike megh-go, `subscribe` takes no plan id: the price row names its plan.

## 5. Limits

- Postgres only, like the rest of megh-rs (the `Table` derive builds Postgres SQL).
- No price arithmetic beyond `Money`: no amount-due, proration or invoices (megh-go has none).

## 6. Invoices and payments (the simple model)

Fields follow HubSpot's invoice, line item and payment objects (the stored amounts, `bill_to`, the refund kept on the payment); product and inventory come from Odoo and Zoho. Module `megh::billing` (`value`, `tax`, `invoice`, `payment`, `draft`, `routes`); every entity is `Entity<Uuid, XData>` keyed by `(org_id, id)` like `Client`; `Address`, `Attributes` and `Name` are those of `booking::value`. Tables `billing_taxes`, `billing_invoices`, `billing_invoice_lines`, `billing_payments` (prefixed so `CREATE TABLE IF NOT EXISTS` cannot meet an application's own `invoices` or `payments`).

- **Values** (`billing::value`): `Micros` (millionths of the invoice's currency), `Currency` (ISO 4217), `Quantity`, `Percent`, `PaymentMethod` (`cash | cheque | card | bank_transfer | other`).
- **`Tax`:** name and a percentage.
- **`Invoice`:** `customer_id` is an org (there is no vendor or client entity here); `number` is `None` while a draft; `status` `draft | open | void`; `date`, `due_date`, `currency`, `bill_to` (the customer's address, copied at `:open`), `comments`; `sub_total`, `tax_total`, `total`, `amount_paid`, `balance_due` (stored, as HubSpot does, so overdue and unpaid are plain `WHERE` clauses). Created only as a draft; changed or deleted only while a draft (409 otherwise); `status`, `customer_id` and `number` are not patched (400).
- **`InvoiceLine`:** a product (no table yet), a service with its appointment, or neither; quantity, unit price, discount percent, tax ids. Locked with its invoice. A line names a product or a service, not both.
- **`Payment`:** one payment pays one open invoice (409 if draft or void; 400 if not the org's); `amount > 0`; `refunded` between nothing and `amount`; `method`, `status` `processing | succeeded | failed | void`; `invoice_id` is not patched.
- **Numbers:** the app passes an `Ids` function (S2) and the result is stored as a `Name`; a counter row per org, kind of document and scope numbers them.
- **Later:** a fixed-amount discount or tax (usually a voucher applied to a payment); credit notes and vendor credits; orders and purchases (with stock); an org's own address, tax id and base currency for the invoice header; exchange rates; payment terms; recurring invoices for customers. Not now, on purpose.
