# TPD — Billing

**Status:** shipped: money and entities (B1), `Subscriptions` (B2), the simple invoice model (S1), numbering (S2), `:open` and `:void` (S3), payments that keep the invoice balance (S4), and products, prices and stock changed by payments (S6). Subscriptions port megh-go's plans, prices, add-ons and subscriptions; megh-go has no price calculation, invoicing or payment-provider calls, so `Subscriptions` has none either (invoices are separate, section 6).
**Modules:** `src/billing/{mod, balance, draft, ids, invoice, invoice_line, lifecycle, payment, subscriptions, tax, totals, value}.rs`, `src/money.rs`; `migrations/0005_billing.sql` (all billing tables) and `migrations/0006_products.sql` (products, prices, stocks, the party columns).
**Related:** `TPD-tenants.md` (a subscription belongs to a tenant).

## 1. Delivery

| # | Feature | Status | PR / issue |
|---|---|---|---|
| B1 | `megh::money` and the billing row types with `Money` accessors | ✅ | #47 / #45 |
| B2 | `Subscriptions`: plans, prices, add-ons, subscribe, change seats, cancel, add-on attach and detach | ✅ | #48 / #46 |
| B3 to B5e | A first, larger document model (vendors, orders, credit notes, applications, payment terms, series). Superseded by S1; not a shipped model, and none of it is in the code. The history is in `CHANGELOG.md` | ✅ | #172 to #184 |
| S1 | Simplify: tax, invoice, invoice line, payment only (section 6); the tables are in `0005_billing.sql`; helpers use `T::TABLE_NAME` | ✅ | #186 / #185 |
| S2 | Numbering (`billing::ids`): the `Ids` trait (`scope`, `id`) the app implements, `Numbering::new(prefix, separator, Middle, padding)` as the ready-made format (`Middle` none, year, year and month, date; a longer format than a `Name` is refused), `issue(connection, tenant_id, Numbered, on, &ids)`; table `billing_counters` (in `0005_billing.sql`), one row per tenant, kind (`invoice`, `payment`) and scope, taken with one `INSERT … ON CONFLICT DO UPDATE … RETURNING` in the caller's transaction, so numbers are never skipped or repeated | ✅ | #188 / #187 |
| S3 | `POST …/invoices/{id}:open` and `:void` (`billing::lifecycle`, like booking's appointment verbs): `:open` needs a line (409), live taxes of the tenant on the lines (400), takes the number from the app's invoice `Ids` (the scope is the invoice date), stores each line's `sub_total` and `total` and the invoice's `sub_total`, `tax_total`, `total`, `amount_paid = 0`, `balance_due = total`, keeps `bill_to` as drafted (it is not copied from the party), in one transaction; `:void` only an open invoice with `amount_paid = 0` (409 otherwise), keeping its number. `billing::totals`: a line's base (quantity times price less the discount), its tax (the sum of its tax percents, on the rounded base) and total are each rounded to the currency's minor unit, half up (`Money::round`); the invoice is the sum of its lines. The app gives the router `Formats` (invoice and payment `Ids`) as an axum `Extension` | ✅ | #190 / #189 |
| S4 | Payments keep the invoice balance (`billing::balance`: `record`, `replace`, `patch`, `remove`, each one transaction that locks the invoice first and refreshes it): `amount_paid` is the sum of `amount` less `refunded` over the processing and succeeded payments, `balance_due` is the total less that; a payment for an invoice that is not open is a 409, one that pays more than is owed a 409 (nothing recorded, no number used); the payment's number comes from the app's payment `Ids` (`Formats`, scope: the payment date) when it is recorded and is never changed (a body that sets `number` is a 400); a PUT or PATCH may not move the payment to another invoice; a refund is a PATCH of `refunded` (there is no refund route); changing or deleting a payment of an invoice that is not open is a 409 | ✅ | #192 / #191 |
| S5 | `parent_id` on a tenant, a customer tenant parented to the seller, creating a `Client` creating its tenant. Superseded: dropped; the other party is a client or a tenant by `party_id` and `party_kind` (section 7) | superseded | — |
| S6 | Products sold with services (section 7): `Client.role` (#228); products, the price ledger and stock, invoice `kind`, party and PO fields, stock changes on payment (`billing::balance`), appointment products (#229); tables and columns in `0006_products.sql` | ✅ | #228, #229 / #227 |

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
pub struct Subscription { id, tenant_id, plan_id, price_id, status, seats, current_period_start, current_period_end, trial_ends_at, cancel_at_period_end, external_id, metadata, created_at, updated_at }
pub struct SubscriptionItem { id, subscription_id, add_on_id, quantity, metadata }

impl PlanPrice { pub fn base(&self) -> Result<Money, MoneyError>; pub fn per_seat(&self) -> Result<Money, MoneyError>; pub fn trial(&self) -> Option<Duration> }
impl AddOn { pub fn price(&self) -> Result<Money, MoneyError> }
```

`PlanPrice::trial_duration` is nanoseconds because megh-go stores a Go `time.Duration` (a real row holds `2592000000000000`, 30 days); `trial()` converts it.

## 4. `Subscriptions` (`billing::subscriptions`, feature `postgres`; #48 / #46)

```rust
impl Subscriptions {                       // holds the injected pool, like Tenants
    pub fn new(pool: PgPool) -> Self;
    pub fn with_lifecycle(self, lifecycle: Option<Lifecycle>) -> Self;   // the app's function, run after each subscription change these commit; its failure is BillingError::Lifecycle
    pub async fn create_plan(&self, plan: &Plan) -> Result<Plan, BillingError>;        // also list_plans, plan(id), plan_by_sku
    pub async fn add_price(&self, price: &PlanPrice) -> Result<PlanPrice, BillingError>;
    pub async fn create_add_on(&self, add_on: &AddOn) -> Result<AddOn, BillingError>;  // also attach_plan_add_on
    pub async fn subscription(&self, tenant_id: Uuid) -> Result<Subscription, BillingError>;
    pub async fn subscribe(&self, tenant_id: Uuid, price_id: Uuid, seats: i64, metadata: Value) -> Result<Subscription, BillingError>;
    pub async fn change_seats(&self, tenant_id: Uuid, seats: i64) -> Result<Subscription, BillingError>;
    pub async fn cancel(&self, tenant_id: Uuid, when: CancelWhen) -> Result<Subscription, BillingError>;   // Now | AtPeriodEnd
    pub async fn attach_add_on(&self, tenant_id: Uuid, add_on_id: Uuid, quantity: i64, metadata: Value) -> Result<Subscription, BillingError>;
    pub async fn detach_add_on(&self, tenant_id: Uuid, add_on_id: Uuid) -> Result<Subscription, BillingError>;
}
pub enum BillingError { NotFound, InvalidSeats, Database(sqlx::Error), Lifecycle(LifecycleError) }   // #[non_exhaustive]
```

There are no REST routes for subscriptions, no plan change, no accessor for a subscription's add-ons and no invoice generation from a subscription. `list_plans` returns the active plans only (`active = true`). megh-go preloads each plan's prices and add-ons and a subscription's add-ons through GORM relations; here every call returns the plain row, (attach and detach return the `Subscription`).

Behaviour follows megh-go: `subscribe` replaces the tenant's subscription (unique per tenant), uses the price's included seats as a minimum, starts a trial (`trialing`, `trial_ends_at`) when the price has a trial, and sets the period end one month or one year ahead; seats are at least 1; an add-on quantity below 1 becomes 1 and re-attaching updates it; `cancel(Now)` sets `canceled`, `cancel(AtPeriodEnd)` sets `cancel_at_period_end`. `subscribe` reads and then upserts (megh-go used a transaction), so two simultaneous first subscriptions of one tenant end with a unique-index error for one of them. Unlike megh-go, `subscribe` takes no plan id: the price row names its plan.

## 5. Limits

- Postgres only, like the rest of megh-rs (the `Table` derive builds Postgres SQL).
- `Subscriptions` does no price arithmetic beyond `Money`: no amount-due, proration or invoices (megh-go has none). Invoice amounts are computed by `billing::totals` (section 6).

## 6. Invoices and payments (the simple model)

Fields follow HubSpot's invoice, line item and payment objects (the stored amounts, `bill_to`, the refund kept on the payment); product and inventory come from Odoo and Zoho. Module `megh::billing`: one file per entity with its routes, as booking does (`tax`, `invoice` with `:open` and `:void`, `invoice_line`, `payment`), the router and OpenAPI in `mod.rs`, and `value`, `ids`, `totals`, `lifecycle`, `balance`, `draft`; every entity is `Entity<Uuid, XData>` keyed by `(tenant_id, id)` like `Client`; `Address`, `Attributes` and `Name` are those of `booking::value`. Tables `billing_taxes`, `billing_invoices`, `billing_invoice_lines`, `billing_payments` (prefixed so `CREATE TABLE IF NOT EXISTS` cannot meet an application's own `invoices` or `payments`).

- **Values** (`billing::value`): `Micros` (millionths of the invoice's currency), `Currency` (ISO 4217), `Quantity`, `Percent`, `PaymentMethod` (`cash | cheque | card | bank_transfer | other`).
- **`Tax`:** name and a percentage.
- **`Invoice`:** the party is set by `party_id` and `party_kind` (section 7); `number` is `None` while a draft; `status` `draft | open | void`; `date`, `due_date`, `currency`, `bill_to` (an address, kept as drafted; `:open` does not copy it), `comments`; `sub_total`, `tax_total`, `total`, `amount_paid`, `balance_due` (stored, as HubSpot does, so overdue and unpaid are plain `WHERE` clauses). Created only as a draft; changed or deleted only while a draft (409 otherwise); `status`, `kind`, `party_id`, `party_kind` and `number` are not patched (400).
- **`InvoiceLine`:** a product (table `products`, section 7), a service with its appointment, or neither; quantity, unit price, discount percent, tax ids. Locked with its invoice. A line names a product or a service, not both.
- **`Payment`:** one payment pays one open invoice (409 if draft or void; 400 if not the tenant's); its number is made when it is recorded; `amount > 0`; `refunded` between nothing and `amount`; `method`, `status` `processing | succeeded | failed | void`; `invoice_id` is not patched.
- **Numbers:** the app passes an `Ids` function (S2) and the result is stored as a `Name`; a row of `billing_counters` per tenant, kind of document and scope numbers them, and a new scope (a new year, say) starts at 1.
- **Later:** a fixed-amount discount or tax (usually a voucher applied to a payment); credit notes and vendor credits; orders and purchases (with stock); a tenant's own address, tax id and base currency for the invoice header; exchange rates; payment terms; recurring invoices for customers. Not now, on purpose.

## 7. Products sold with services (S6, #227)

Products bought and sold alongside the services booking has: the colour used in a hair colour appointment, a tooth in a dental RCT. Not full inventory management: there are no stock moves; payments change the stock. Invoice, bill and PO follow Xero (one invoice with a type; a PO is a payable draft).

**Booking** (`src/booking`):

```rust
pub role: Option<Name>,                 // on ClientData, as on ProviderData: "patient", "supplier"

#[table(name = "products", keys = ["tenant_id", "id"])]
pub struct ProductData {
    pub tenant_id: Uuid,                // the owner
    pub sku: Name,                      // unique per owner
    pub name: Name,
    pub description: String,
    pub unit: Name,                     // what one of quantity counts: tube, ml, piece
    pub price: Micros,                  // the current price of one unit
    pub currency: Currency,
    pub attributes: Attributes,         // grouping, as for a service
}

pub unit: Name,                         // on ServiceData, "session" unless given
pub price: Micros,                      // on ServiceData, required on create
pub currency: Currency,

#[table(name = "stocks", keys = ["tenant_id", "id"])]
pub struct StockData {
    pub tenant_id: Uuid,                // the tenant holding it; the product may be another tenant's
    pub product_id: Uuid,
    pub on_hand: Quantity,              // read-only; changed only by payments
}

#[table(name = "prices", keys = ["tenant_id", "id"])]
pub struct PriceData {
    pub tenant_id: Uuid,
    pub item_id: Uuid,
    pub item_kind: ItemKind,            // Product | Service
    pub price: Micros,
    pub currency: Currency,
    pub effective_at: DateTime<Utc>,
}

pub products: Vec<Used>,                // on AppointmentData, a JSON column, default []
pub struct Used { pub product_id: Uuid, pub quantity: Quantity }
```

- A product or service holds its current `price` and `unit`. `Price` is the ledger of its prices: a create, and a `PUT` or `PATCH` that changes `price` or `currency`, adds a row effective at that moment, in the same transaction. Nobody writes the ledger directly; rows are never changed or deleted.
- Routes: products CRUD (a repeated SKU of the owner is a 409) and `GET /products/{id}:stock`, the tenant's stock of it; `GET /prices`, `/prices/{id}` and `GET /stocks`, `/stocks/{id}`, read only. A stock row has its own id and one row per tenant and product.
- Services stored before prices cost 0 USD a session, with no ledger row until their price changes.
- Booking an appointment checks each product against the tenant's `on_hand` (409 if too little), with the provider and resource fit. A PATCH of `products` is checked the same way. A booked appointment holds no stock. The app enters its invoice lines at the item's current price.

**Billing** (`src/billing`):

```rust
pub kind: InvoiceKind,                  // on InvoiceData: Receivable | Payable
pub party_id: Uuid,                     // replaces customer_id
pub party_kind: PartyKind,              // Client | Tenant
pub reference: String,                  // the PO number, or the supplier's bill number
pub delivery_date: Option<NaiveDate>,
pub deliver_to: Option<Address>,
```

- The other party is a client (a supplier is a client with a role) or a tenant. A PO is a payable invoice that is still a draft; it has no entity of its own.
- `Payment` is unchanged; it is the receipt, one per invoice in practice. When it becomes `succeeded`, a receivable invoice's product lines leave the invoice tenant's stock (409 if too little) and a payable invoice's enter it. A full refund, `failed`, `void` or a deleted payment undoes that change. The products move once per invoice: while it has a succeeded payment not refunded in full. In the transaction that already locks the invoice and refreshes its balance.

**Migration `0006_products.sql`**: tables `products`, `stocks`, `prices`; `clients.role` (nullable, existing clients have none); `services.unit`, `price`, `currency` (defaults `session`, 0, `USD`); `appointments.products` (default `[]`); on `billing_invoices`, `customer_id` renamed `party_id` (without its tenant foreign key; its index is renamed `billing_invoices_party`), new `party_kind`, `kind`, `reference`, `delivery_date`, `deliver_to`, existing rows `party_kind = 'tenant'` and `kind = 'receivable'`.

## Decided (from the clarifications)

- `totals::invoice` returns `Result<Amounts, TotalsError>`; `TooLarge` on overflow.
- `subscribe` refuses fewer than 1 seat (`InvalidSeats`), as `change_seats` does.
- A `PUT` that changes a line's `invoice_id` is a 400, as `PATCH` is; a line never changes invoice.
- Invoice writes run in a transaction in which `ensure_draft` locks the invoice `FOR UPDATE` and line checks lock it `FOR SHARE`.
