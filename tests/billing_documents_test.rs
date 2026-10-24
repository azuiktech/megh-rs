//! B4 (docs/TPD-billing.md): the billing tables store and return every entity, and refuse what the types say cannot be.

use chrono::NaiveDate;
use megh::billing::invoice::{InvoiceData, InvoiceStatus};
use megh::billing::invoice_line::InvoiceLineData;
use megh::billing::payment::{PaymentData, PaymentStatus};
use megh::billing::tax::TaxData;
use megh::billing::value::{Currency, Micros, PaymentMethod, Percent, Quantity};
use megh::booking::value::{Address, Attributes, Name};
use megh::{Entity, Table, TableEntity};
use rust_decimal::Decimal;
use serde::Serialize;
use sqlx::PgPool;
use uuid::Uuid;

async fn org(pool: &PgPool, name: &str) -> Uuid {
    sqlx::query_scalar("INSERT INTO organizations (name) VALUES ($1) RETURNING id").bind(name).fetch_one(pool).await.unwrap()
}

async fn service(pool: &PgPool, org: Uuid) -> Uuid {
    sqlx::query_scalar(r#"INSERT INTO services (org_id, id, name, steps) VALUES ($1, $2, 'Cleaning', '[{"duration": "PT30M"}]') RETURNING id"#)
        .bind(org)
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

async fn store<T>(pool: &PgPool, data: T) -> Result<Entity<Uuid, T>, sqlx::Error>
where
    T: Table + Serialize + for<'r> sqlx::FromRow<'r, sqlx::postgres::PgRow> + Send + Unpin,
{
    TableEntity::insert(&Entity::new(Uuid::now_v7(), data), pool).await
}

fn invoice(org: Uuid, customer: Uuid) -> InvoiceData {
    InvoiceData {
        org_id: org,
        customer_id: customer,
        number: None,
        status: InvoiceStatus::Draft,
        date: day(),
        due_date: day(),
        currency: Currency::try_new("usd").unwrap(),
        bill_to: None,
        comments: String::new(),
        sub_total: Micros::default(),
        tax_total: Micros::default(),
        total: Micros::default(),
        amount_paid: Micros::default(),
        balance_due: Micros::default(),
        attributes: Attributes::new(),
    }
}

fn line(org: Uuid, invoice_id: Uuid) -> InvoiceLineData {
    InvoiceLineData {
        org_id: org,
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

fn payment(org: Uuid, invoice_id: Uuid) -> PaymentData {
    PaymentData {
        org_id: org,
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
    let (mine, customer) = (org(&pool, "Clinic").await, org(&pool, "Ada").await);

    let tax = TaxData { org_id: mine, name: name("Sales tax"), rate: Percent::try_new(Decimal::new(75, 1)).unwrap(), description: String::new(), attributes: Attributes::new() };
    assert_eq!(store(&pool, tax.clone()).await.unwrap().data, tax);

    let address = Address { line_1: name("1 Main St"), line_2: None, city: name("Austin"), region: name("TX"), postal_code: name("78701"), country: celes::Country::from_alpha2("US").unwrap() };
    let sent = InvoiceData {
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
    assert_eq!(stored.data, sent);

    let billed = InvoiceLineData { service_id: Some(service(&pool, mine).await), tax_ids: vec![Uuid::now_v7()], discount: Percent::try_new(Decimal::new(10, 0)).unwrap(), ..line(mine, stored.id) };
    assert_eq!(store(&pool, billed.clone()).await.unwrap().data, billed);

    let paid = PaymentData { number: Some(name("PAY-0001")), refunded: Micros::new(5_000_000), ..payment(mine, stored.id) };
    assert_eq!(store(&pool, paid.clone()).await.unwrap().data, paid);
}

#[sqlx::test(migrations = false)]
async fn the_database_refuses_what_the_types_cannot_be(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let (mine, customer) = (org(&pool, "Clinic").await, org(&pool, "Ada").await);
    let owed = store(&pool, invoice(mine, customer)).await.unwrap();

    let both = InvoiceLineData { product_id: Some(Uuid::now_v7()), service_id: Some(service(&pool, mine).await), ..line(mine, owed.id) };
    assert!(store(&pool, both).await.is_err(), "a line names a product or a service, not both");
    assert!(store(&pool, invoice(mine, Uuid::now_v7())).await.is_err(), "a customer is an org");
    assert!(store(&pool, line(mine, Uuid::now_v7())).await.is_err(), "a line belongs to an invoice of the org");
    assert!(store(&pool, payment(mine, Uuid::now_v7())).await.is_err(), "a payment is for an invoice of the org");
    assert!(store(&pool, PaymentData { amount: Micros::new(0), ..payment(mine, owed.id) }).await.is_err(), "a payment is of something");
    assert!(store(&pool, PaymentData { refunded: Micros::new(120_000_001), ..payment(mine, owed.id) }).await.is_err(), "no more is refunded than was paid");
    assert!(store(&pool, PaymentData { refunded: Micros::new(-1), ..payment(mine, owed.id) }).await.is_err());
}

#[sqlx::test(migrations = false)]
async fn a_number_is_used_once_per_org(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let (mine, other, customer) = (org(&pool, "Clinic").await, org(&pool, "Other").await, org(&pool, "Ada").await);
    let numbered = |org: Uuid| InvoiceData { number: Some(name("INV-0001")), ..invoice(org, customer) };
    let first = store(&pool, numbered(mine)).await.unwrap();
    assert!(store(&pool, numbered(mine)).await.is_err());
    assert!(store(&pool, numbered(other)).await.is_ok(), "another org has its own numbers");
    assert!(store(&pool, invoice(mine, customer)).await.is_ok() && store(&pool, invoice(mine, customer)).await.is_ok(), "drafts have no number");
    let receipt = |number: &str| PaymentData { number: Some(name(number)), ..payment(mine, first.id) };
    assert!(store(&pool, receipt("PAY-1")).await.is_ok());
    assert!(store(&pool, receipt("PAY-1")).await.is_err());
}

#[sqlx::test(migrations = false)]
async fn deleting_an_invoice_deletes_its_lines_and_payments(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let (mine, customer) = (org(&pool, "Clinic").await, org(&pool, "Ada").await);
    let owed = store(&pool, invoice(mine, customer)).await.unwrap();
    let billed = store(&pool, line(mine, owed.id)).await.unwrap();
    let paid = store(&pool, payment(mine, owed.id)).await.unwrap();
    TableEntity::soft_delete(&owed, &pool).await.unwrap().unwrap();
    for (table, id) in [("billing_invoice_lines", billed.id), ("billing_payments", paid.id)] {
        let deleted: bool = sqlx::query_scalar(&format!("SELECT deleted_at IS NOT NULL FROM {table} WHERE id = $1")).bind(id).fetch_one(&pool).await.unwrap();
        assert!(deleted, "{table}");
    }
    let kept: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM organizations WHERE id = $1)").bind(customer).fetch_one(&pool).await.unwrap();
    assert!(kept, "the customer org stays");
}

#[sqlx::test(migrations = false)]
async fn the_old_billing_tables_are_dropped_once(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    sqlx::raw_sql("CREATE TABLE billing_orders (id UUID); INSERT INTO billing_orders VALUES (gen_random_uuid())").execute(&pool).await.unwrap();
    let mine = org(&pool, "Clinic").await;
    let owed = store(&pool, invoice(mine, org(&pool, "Ada").await)).await.unwrap();
    megh::migrate(&pool).await.unwrap();
    let old: bool = sqlx::query_scalar("SELECT to_regclass('billing_orders') IS NOT NULL").fetch_one(&pool).await.unwrap();
    assert!(!old, "the first shape of 0017 is cleaned up");
    megh::migrate(&pool).await.unwrap();
    let kept: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM billing_invoices WHERE id = $1)").bind(owed.id).fetch_one(&pool).await.unwrap();
    assert!(!kept, "the invoices of the old shape went with it, and a new run keeps what is stored");
}
