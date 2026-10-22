//! B4 (docs/TPD-billing.md section 6): the billing document tables store and return every entity, and refuse what the
//! types say cannot be.

use chrono::NaiveDate;
use megh::billing::invoice::{InvoiceData, InvoiceKind, InvoiceLineData, InvoiceStatus};
use megh::billing::order::{OrderData, OrderKind, OrderLineData, OrderStatus};
use megh::billing::payment::{ApplicationData, PaymentData, PaymentKind, PaymentStatus};
use megh::billing::tax::{PaymentTermData, SeriesData, TaxData};
use megh::billing::value::{Currency, Days, ExchangeRate, Micros, Percent, PaymentMethod, Quantity, TaxMode};
use megh::billing::vendor::VendorData;
use megh::booking::value::{Address, Attributes, Contact, Email, Name};
use megh::{Entity, Table, TableEntity};
use rust_decimal::Decimal;
use serde::Serialize;
use sqlx::PgPool;
use uuid::Uuid;

async fn org(pool: &PgPool) -> Uuid {
    sqlx::query_scalar("INSERT INTO organizations (name) VALUES ('Clinic') RETURNING id").fetch_one(pool).await.unwrap()
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

fn usd() -> Currency {
    Currency::try_new("usd").unwrap()
}

async fn store<T>(pool: &PgPool, data: T) -> Result<Entity<Uuid, T>, sqlx::Error>
where
    T: Table + Serialize + for<'r> sqlx::FromRow<'r, sqlx::postgres::PgRow> + Send + Unpin,
{
    TableEntity::insert(&Entity::new(Uuid::now_v7(), data), pool).await
}

fn invoice(org: Uuid, kind: InvoiceKind) -> InvoiceData {
    InvoiceData {
        org_id: org,
        kind,
        party_id: Uuid::now_v7(),
        number: None,
        status: InvoiceStatus::Draft,
        date: day(),
        due_date: day(),
        payment_term_id: None,
        reference: None,
        currency: usd(),
        exchange_rate: ExchangeRate::default(),
        tax_mode: TaxMode::Exclusive,
        billing_address: None,
        shipping_address: None,
        order_id: None,
        credited_invoice_id: None,
        location_id: None,
        notes: String::new(),
        terms: String::new(),
        sub_total: Micros::default(),
        tax_total: Micros::default(),
        total: Micros::default(),
        attributes: Attributes::new(),
    }
}

fn invoice_line(org: Uuid, invoice_id: Uuid) -> InvoiceLineData {
    InvoiceLineData {
        org_id: org,
        invoice_id,
        position: 1,
        product_id: None,
        service_id: None,
        appointment_id: None,
        order_line_id: None,
        description: name("Tooth cleaning"),
        quantity: Quantity::try_new(Decimal::new(15, 1)).unwrap(),
        unit: None,
        unit_price: Micros::new(80_000_000),
        discount: Percent::default(),
        tax_ids: vec![],
        sub_total: Micros::default(),
        total: Micros::default(),
    }
}

fn payment(org: Uuid) -> PaymentData {
    PaymentData {
        org_id: org,
        kind: PaymentKind::Inbound,
        party_id: Uuid::now_v7(),
        number: None,
        date: day(),
        amount: Micros::new(120_000_000),
        currency: usd(),
        exchange_rate: ExchangeRate::default(),
        method: PaymentMethod::Card,
        status: PaymentStatus::Succeeded,
        reference: None,
        description: String::new(),
        fees: Micros::new(3_000_000),
        location_id: None,
        attributes: Attributes::new(),
    }
}

#[sqlx::test(migrations = false)]
async fn every_entity_is_stored_and_read_back(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let org = org(&pool).await;

    let tax = TaxData { org_id: org, name: name("Sales tax"), rate: Percent::try_new(Decimal::new(75, 1)).unwrap(), description: String::new(), attributes: Attributes::new() };
    assert_eq!(store(&pool, tax.clone()).await.unwrap().data, tax);

    let term = PaymentTermData { org_id: org, name: name("Net 30"), net_days: Days::try_new(30).unwrap(), description: String::new() };
    let term = store(&pool, term.clone()).await.unwrap();
    assert_eq!(term.net_days, Days::try_new(30).unwrap());

    let series = SeriesData { org_id: org, name: name("Invoices"), prefix: "INV-".into(), next: 1, padding: 4 };
    assert_eq!(store(&pool, series.clone()).await.unwrap().data, series);

    let vendor = VendorData {
        org_id: org,
        name: name("Acme Supplies"),
        description: String::new(),
        contact: Contact { email: Some(Email::try_new("ap@acme.example").unwrap()), ..Contact::default() },
        address: Some(Address { line_1: name("1 Main St"), line_2: None, city: name("Austin"), region: name("TX"), postal_code: name("78701"), country: celes::Country::from_alpha2("US").unwrap() }),
        tax_id: Some(name("12-3456789")),
        payment_term_id: Some(term.id),
        currency: Some(usd()),
        attributes: Attributes::new(),
    };
    assert_eq!(store(&pool, vendor.clone()).await.unwrap().data, vendor);

    let purchase = OrderData {
        org_id: org,
        kind: OrderKind::PurchaseOrder,
        party_id: Uuid::now_v7(),
        number: Some(name("PO-0001")),
        status: OrderStatus::Draft,
        date: day(),
        expected_date: Some(day()),
        reference: None,
        currency: usd(),
        exchange_rate: ExchangeRate::default(),
        payment_term_id: Some(term.id),
        tax_mode: TaxMode::Inclusive,
        billing_address: None,
        shipping_address: None,
        location_id: Some(org),
        delivery_method: Some(name("Courier")),
        notes: "n".into(),
        terms: "t".into(),
        sub_total: Micros::new(1),
        tax_total: Micros::new(2),
        total: Micros::new(3),
        attributes: Attributes::new(),
    };
    let order = store(&pool, purchase.clone()).await.unwrap();
    assert_eq!(order.data, purchase);
    let line = OrderLineData {
        org_id: org,
        order_id: order.id,
        position: 1,
        product_id: Some(Uuid::now_v7()),
        service_id: None,
        description: name("Gloves"),
        quantity: Quantity::try_new(Decimal::new(100, 0)).unwrap(),
        unit: Some(name("box")),
        unit_price: Micros::new(5_000_000),
        discount: Percent::try_new(Decimal::new(10, 0)).unwrap(),
        tax_ids: vec![Uuid::now_v7()],
        expected_date: None,
        sub_total: Micros::default(),
        total: Micros::default(),
    };
    assert_eq!(store(&pool, line.clone()).await.unwrap().data, line);

    let bill = store(&pool, InvoiceData { order_id: Some(order.id), number: Some(name("B-1")), ..invoice(org, InvoiceKind::Bill) }).await.unwrap();
    assert_eq!(bill.kind, InvoiceKind::Bill);
    let line = invoice_line(org, bill.id);
    assert_eq!(store(&pool, line.clone()).await.unwrap().data, line);

    let paid = store(&pool, payment(org)).await.unwrap();
    let application = ApplicationData { org_id: org, invoice_id: bill.id, payment_id: Some(paid.id), credit_id: None, amount: Micros::new(50_000_000) };
    assert_eq!(store(&pool, application.clone()).await.unwrap().data, application);
}

#[sqlx::test(migrations = false)]
async fn a_line_names_at_most_one_of_product_and_service(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let org = org(&pool).await;
    let owner = store(&pool, invoice(org, InvoiceKind::Invoice)).await.unwrap();
    let both = InvoiceLineData { product_id: Some(Uuid::now_v7()), service_id: Some(service(&pool, org).await), ..invoice_line(org, owner.id) };
    assert!(store(&pool, both).await.is_err());
}

#[sqlx::test(migrations = false)]
async fn an_application_is_of_exactly_one_payment_or_credit(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let org = org(&pool).await;
    let owed = store(&pool, invoice(org, InvoiceKind::Invoice)).await.unwrap();
    let credit = store(&pool, invoice(org, InvoiceKind::CreditNote)).await.unwrap();
    let paid = store(&pool, payment(org)).await.unwrap();
    let application = |payment_id, credit_id| ApplicationData { org_id: org, invoice_id: owed.id, payment_id, credit_id, amount: Micros::new(1_000_000) };
    assert!(store(&pool, application(None, None)).await.is_err());
    assert!(store(&pool, application(Some(paid.id), Some(credit.id))).await.is_err());
    assert!(store(&pool, application(Some(paid.id), None)).await.is_ok());
    assert!(store(&pool, application(None, Some(credit.id))).await.is_ok());
}

#[sqlx::test(migrations = false)]
async fn only_a_credit_note_or_vendor_credit_credits_an_invoice(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let org = org(&pool).await;
    let owed = store(&pool, invoice(org, InvoiceKind::Invoice)).await.unwrap();
    let crediting = |kind| InvoiceData { credited_invoice_id: Some(owed.id), ..invoice(org, kind) };
    assert!(store(&pool, crediting(InvoiceKind::Invoice)).await.is_err());
    assert!(store(&pool, crediting(InvoiceKind::Bill)).await.is_err());
    assert!(store(&pool, crediting(InvoiceKind::CreditNote)).await.is_ok());
    assert!(store(&pool, crediting(InvoiceKind::VendorCredit)).await.is_ok());
}

#[sqlx::test(migrations = false)]
async fn a_number_is_used_once_per_org_and_kind(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let org = org(&pool).await;
    let numbered = |kind| InvoiceData { number: Some(name("0001")), ..invoice(org, kind) };
    assert!(store(&pool, numbered(InvoiceKind::Invoice)).await.is_ok());
    assert!(store(&pool, numbered(InvoiceKind::Invoice)).await.is_err());
    assert!(store(&pool, numbered(InvoiceKind::Bill)).await.is_ok());
}

#[sqlx::test(migrations = false)]
async fn deleting_an_invoice_deletes_its_lines_and_applications(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let org = org(&pool).await;
    let owed = store(&pool, invoice(org, InvoiceKind::Invoice)).await.unwrap();
    let line = store(&pool, invoice_line(org, owed.id)).await.unwrap();
    let paid = store(&pool, payment(org)).await.unwrap();
    let application = store(&pool, ApplicationData { org_id: org, invoice_id: owed.id, payment_id: Some(paid.id), credit_id: None, amount: Micros::new(1_000_000) }).await.unwrap();
    TableEntity::soft_delete(&owed, &pool).await.unwrap().unwrap();
    for (table, id) in [("billing_invoice_lines", line.id), ("billing_applications", application.id)] {
        let deleted: bool = sqlx::query_scalar(&format!("SELECT deleted_at IS NOT NULL FROM {table} WHERE id = $1")).bind(id).fetch_one(&pool).await.unwrap();
        assert!(deleted, "{table}");
    }
    let kept: bool = sqlx::query_scalar("SELECT deleted_at IS NULL FROM billing_payments WHERE id = $1").bind(paid.id).fetch_one(&pool).await.unwrap();
    assert!(kept);
}
