# TPD — Billing

**Status:** B1 (money and entities) and B2 (`Subscriptions` lifecycle) shipped. Ports megh-go's plans, prices, add-ons and subscriptions; megh-go has no price calculation, invoicing or payment-provider calls, so neither does this.
**Modules:** `src/billing/mod.rs`, `src/billing/subscriptions.rs`, `src/money.rs`, `migrations/0006_align_org.sql` (the tables).
**Related:** `TPD-organizations.md` (the billing tables were created there, schema only).

## 1. Delivery

| # | Feature | Status | PR / issue |
|---|---|---|---|
| B1 | `megh::money` and the billing row types with `Money` accessors | `[x]` | #47 / #45 |
| B2 | `Subscriptions`: plans, prices, add-ons, subscribe, change seats, cancel, add-on attach and detach | `[x]` | #48 / #46 |
| B3 | Document types (section 6): values and entities, no tables yet | `[x]` | #172 / #171 |
| B4 | Migration `0017_billing_documents.sql`: the ten tables, named `billing_taxes`, `billing_payment_terms`, `billing_series`, `billing_vendors`, `billing_orders`, `billing_order_lines`, `billing_invoices`, `billing_invoice_lines`, `billing_payments`, `billing_applications` (a library's tables prefixed so `CREATE TABLE IF NOT EXISTS` cannot silently meet an application's `orders` or `payments`), composite keys, CHECKs; test stores and reads back every entity | `[x]` | #174 / #173 |
| B5a | `SignedIn` and `Granted` OpenAPI modifiers move from `booking` to `megh::auth::openapi`, shared with the billing router | `[x]` | #176 / #175 |
| B5b | CRUD routes (like Client, `/orgs/{org_id}/…`, lowerCamelCase) for `taxes`, `paymentTerms`, `series`, `vendors`, one file each in `src/billing/routes`; `megh::billing::{router, openapi}`; `examples/booking.rs` mounts booking and billing; `Series.next >= 1` and `padding >= 0` are checked on input (400) | `[x]` | #178 / #177 |
| B5c | `orders`, `orderLines` (`src/billing/routes`, helpers `billing::party` and `billing::draft`): an order is created only as a `draft` (400 otherwise); PUT, PATCH and DELETE only while draft (409); its lines are locked with it; `party_id` must be a live client (sales order) or vendor (purchase order) of the org (400) on create and PUT; PATCH may not set `kind`, `party_id` or `status` (400); a line's `order_id` and `service_id` must belong to the org (400), `order_id` is not patched, a line names a product or a service, not both (400); static SQL per `Document` | `[~]` | #180 / #179 |
| B5d | `invoices`, `invoiceLines`, same rules | `[ ]` | — |
| B5e | `payments`, `applications` | `[ ]` | — |
| B6 | `:open`, `:void`, numbering from `Series`, totals and tax rounding, applying payments | `[ ]` | — |

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

## 6. Documents: orders, invoices, payments (B3, types only)

What customers owe the org and what the org owes vendors. Fields come from Odoo (`account.move`, `account.payment`, `sale.order`, `purchase.order`), Zoho Inventory (Invoice, Bill, Credit Note, Vendor Credit, Sales Order, Purchase Order, Payment) and HubSpot commerce (`invoices`, `line_items`, `commerce_payments`; sell side only). Module `megh::billing` (`value`, `tax`, `vendor`, `order`, `invoice`, `payment`); every entity is `Entity<Uuid, XData>` keyed by `(org_id, id)` like `Client`; `Address`, `Contact`, `Name` and `Attributes` are those of `booking::value` (a common module is for later).

- **Values** (`billing::value`): `Micros` (millionths of the document's currency, as the other billing tables), `Currency` (ISO 4217 by `rusty_money::iso`), `Quantity`, `Percent`, `ExchangeRate` (rust_decimal), `Days`, `TaxMode`, `PaymentMethod` (`cash | cheque | card | bank_transfer | other`).
- **Setup:** `Tax` (name, percent rate), `PaymentTerm` (net days), `Series` (prefix, next, padding), `Vendor` (name, contact, address, tax id, payment term, currency). The customer is `booking::client::Client`.
- **`Order` + `OrderLine`:** `kind` `sales_order | purchase_order`, status `draft | open | cancelled` (a draft is a quote); `party_id` is the client or the vendor.
- **`Invoice` + `InvoiceLine`:** one entity, `kind` `invoice | credit_note | bill | vendor_credit` (Zoho's names); status `draft | open | void`. Paid, partly paid and overdue are derived from applications and `due_date`, never stored. An open invoice is not changed; a credit note or vendor credit (`credited_invoice_id`) corrects it. Lines name at most one of product or service (and the appointment).
- **`Payment` + `Application`:** `kind` `inbound | outbound` (Odoo), status `processing | succeeded | failed | void`; a refund is a payment the other way. An application sets part of a payment or a credit against an invoice.
- **Money:** exact `Decimal` and `Micros`; a percent tax on a price needs rounding to the currency's minor unit, done with `rusty_money::Money::round` (Odoo offers per line or once overall, default once). The choice is open.
- **Left out for now:** compound and fixed taxes, tax groups, payment-term instalments, header discounts and shipping (extra lines), region tax fields, gateways, e-mail and PDF tracking, recurring invoices, chart of accounts. Needs sqlx and nutype `rust_decimal` features.
