//! B4 (docs/TPD-billing.md): the billing tables store and return every entity, and refuse what the types say cannot be.

use chrono::{NaiveDate, Utc};
use megh::billing::invoice::{Invoice, InvoiceKind, InvoiceStatus, PartyKind};
use megh::billing::invoice_line::InvoiceLine;
use megh::billing::payment::{Payment, PaymentStatus};
use megh::billing::tax::Tax;
use megh::billing::value::{Currency, Micros, PaymentMethod, Percent, Quantity};
use megh::booking::value::{Address, Attributes, Name};
use megh::auth::rest::{ApiResult, Key};
use megh::auth::crud::{self, Row};
use rust_decimal::Decimal;
use sqlx::PgPool;
use uuid::Uuid;

async fn tenant(pool: &PgPool, name: &str) -> Uuid {
    sqlx::query_scalar("INSERT INTO tenants (name) VALUES ($1) RETURNING id").bind(name).fetch_one(pool).await.unwrap()
}

async fn service(pool: &PgPool, tenant: Uuid) -> Uuid {
    sqlx::query_scalar(r#"INSERT INTO services (tenant_id, id, name, steps) VALUES ($1, $2, 'Cleaning', '[{"duration": "PT30M"}]') RETURNING id"#)
        .bind(tenant)
        .bind(Uuid::now_v7())
        .fetch_one(pool)
        .await
        .unwrap()
}

fn name(text: &str) -> Name {
    Name::try_new(text).unwrap()
}

fn day() -> NaiveDate {
    NaiveDate::from_ymd_opt(2026, 10, 22).unwrap()
}

async fn store<T: Row>(pool: &PgPool, data: T) -> ApiResult<T> {
    crud::create(pool, data, None).await
}

fn invoice(tenant: Uuid, customer: Uuid) -> Invoice {
    Invoice {
        tenant_id: tenant,
        id: Uuid::nil(),
        created_at: Utc::now(),
        updated_at: None,
        deleted_at: None,
        kind: InvoiceKind::Receivable,
        party_id: customer,
        party_kind: PartyKind::Tenant,
        number: None,
        status: InvoiceStatus::Draft,
        date: day(),
        due_date: day(),
        currency: Currency::try_new("usd").unwrap(),
        bill_to: None,
        comments: String::new(),
        reference: String::new(),
        delivery_date: None,
        deliver_to: None,
        sub_total: Micros::default(),
        tax_total: Micros::default(),
        total: Micros::default(),
        amount_paid: Micros::default(),
        balance_due: Micros::default(),
        attributes: Attributes::new(),
    }
}

fn line(tenant: Uuid, invoice_id: Uuid) -> InvoiceLine {
    InvoiceLine {
        tenant_id: tenant,
        id: Uuid::nil(),
        created_at: Utc::now(),
        updated_at: None,
        deleted_at: None,
        invoice_id,
        position: 1,
        product_id: None,
        service_id: None,
        appointment_id: None,
        description: name("Tooth cleaning"),
        quantity: Quantity::try_new(Decimal::new(15, 1)).unwrap(),
        unit_price: Micros::new(80_000_000),
        discount: Percent::default(),
        tax_ids: vec![],
        sub_total: Micros::default(),
        total: Micros::default(),
    }
}

fn payment(tenant: Uuid, invoice_id: Uuid) -> Payment {
    Payment {
        tenant_id: tenant,
        id: Uuid::nil(),
        created_at: Utc::now(),
        updated_at: None,
        deleted_at: None,
        invoice_id,
        number: None,
        date: day(),
        amount: Micros::new(120_000_000),
        method: PaymentMethod::Card,
        status: PaymentStatus::Succeeded,
        refunded: Micros::default(),
        attributes: Attributes::new(),
    }
}

#[sqlx::test(migrations = false)]
async fn every_entity_is_stored_and_read_back(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let (mine, customer) = (tenant(&pool, "Clinic").await, tenant(&pool, "Ada").await);

    let tax = Tax { id: Uuid::nil(), created_at: Utc::now(), updated_at: None, deleted_at: None, tenant_id: mine, name: name("Sales tax"), rate: Percent::try_new(Decimal::new(75, 1)).unwrap(), description: String::new(), attributes: Attributes::new() };
    let stored = store(&pool, tax.clone()).await.unwrap();
    assert_eq!(stored, Tax { id: stored.id, created_at: stored.created_at, updated_at: stored.updated_at, ..tax });

    let address = Address { line_1: name("1 Main St"), line_2: None, city: name("Austin"), region: name("TX"), postal_code: name("78701"), country: celes::Country::from_alpha2("US").unwrap() };
    let sent = Invoice {
        number: Some(name("INV-0001")),
        status: InvoiceStatus::Open,
        bill_to: Some(address),
        comments: "thank you".into(),
        total: Micros::new(120_000_000),
        amount_paid: Micros::new(20_000_000),
        balance_due: Micros::new(100_000_000),
        ..invoice(mine, customer)
    };
    let stored = store(&pool, sent.clone()).await.unwrap();
    assert_eq!(stored, Invoice { id: stored.id, created_at: stored.created_at, updated_at: stored.updated_at, ..sent });

    let billed = InvoiceLine { service_id: Some(service(&pool, mine).await), tax_ids: vec![Uuid::now_v7()], discount: Percent::try_new(Decimal::new(10, 0)).unwrap(), ..line(mine, stored.id) };
    let line = store(&pool, billed.clone()).await.unwrap();
    assert_eq!(line, InvoiceLine { id: line.id, created_at: line.created_at, updated_at: line.updated_at, ..billed });

    let paid = Payment { number: Some(name("PAY-0001")), refunded: Micros::new(5_000_000), ..payment(mine, stored.id) };
    let receipt = store(&pool, paid.clone()).await.unwrap();
    assert_eq!(receipt, Payment { id: receipt.id, created_at: receipt.created_at, updated_at: receipt.updated_at, ..paid });
}

#[sqlx::test(migrations = false)]
async fn the_database_refuses_what_the_types_cannot_be(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let (mine, customer) = (tenant(&pool, "Clinic").await, tenant(&pool, "Ada").await);
    let owed = store(&pool, invoice(mine, customer)).await.unwrap();

    let both = InvoiceLine { product_id: Some(Uuid::now_v7()), service_id: Some(service(&pool, mine).await), ..line(mine, owed.id) };
    assert!(store(&pool, both).await.is_err(), "a line names a product or a service, not both");
    assert!(store(&pool, line(mine, Uuid::now_v7())).await.is_err(), "a line belongs to an invoice of the tenant");
    assert!(store(&pool, payment(mine, Uuid::now_v7())).await.is_err(), "a payment is for an invoice of the tenant");
    assert!(store(&pool, Payment { amount: Micros::new(0), ..payment(mine, owed.id) }).await.is_err(), "a payment is of something");
    assert!(store(&pool, Payment { refunded: Micros::new(120_000_001), ..payment(mine, owed.id) }).await.is_err(), "no more is refunded than was paid");
    assert!(store(&pool, Payment { refunded: Micros::new(-1), ..payment(mine, owed.id) }).await.is_err());
}

#[sqlx::test(migrations = false)]
async fn a_number_is_used_once_per_tenant(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let (mine, other, customer) = (tenant(&pool, "Clinic").await, tenant(&pool, "Other").await, tenant(&pool, "Ada").await);
    let numbered = |tenant: Uuid| Invoice { number: Some(name("INV-0001")), ..invoice(tenant, customer) };
    let first = store(&pool, numbered(mine)).await.unwrap();
    assert!(store(&pool, numbered(mine)).await.is_err());
    assert!(store(&pool, numbered(other)).await.is_ok(), "another tenant has its own numbers");
    assert!(store(&pool, invoice(mine, customer)).await.is_ok() && store(&pool, invoice(mine, customer)).await.is_ok(), "drafts have no number");
    let receipt = |number: &str| Payment { number: Some(name(number)), ..payment(mine, first.id) };
    assert!(store(&pool, receipt("PAY-1")).await.is_ok());
    assert!(store(&pool, receipt("PAY-1")).await.is_err());
}

#[sqlx::test(migrations = false)]
async fn deleting_an_invoice_deletes_its_lines_and_payments(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let (mine, customer) = (tenant(&pool, "Clinic").await, tenant(&pool, "Ada").await);
    let owed = store(&pool, invoice(mine, customer)).await.unwrap();
    let billed = store(&pool, line(mine, owed.id)).await.unwrap();
    let paid = store(&pool, payment(mine, owed.id)).await.unwrap();
    crud::delete::<Invoice, _>(&pool, &Key { tenant_id: mine, id: owed.id }, None).await.unwrap();
    for (table, id) in [("billing_invoice_lines", billed.id), ("billing_payments", paid.id)] {
        let deleted: bool = sqlx::query_scalar(&format!("SELECT deleted_at IS NOT NULL FROM {table} WHERE id = $1")).bind(id).fetch_one(&pool).await.unwrap();
        assert!(deleted, "{table}");
    }
    let kept: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM tenants WHERE id = $1)").bind(customer).fetch_one(&pool).await.unwrap();
    assert!(kept, "the customer tenant stays");
}
