//! S2 (docs/TPD-billing.md): the ids of invoices and payments. The app supplies an `Ids` function; `Numbering` is the
//! ready-made format (prefix, separator, optional date part, padded sequence); a counter per tenant, kind of document and
//! scope numbers them.

use chrono::NaiveDate;
use megh::billing::ids::{issue, Ids, Middle, Numbered, Numbering, NumberingError};
use megh::booking::value::Name;
use sqlx::PgPool;
use uuid::Uuid;

fn day(year: i32, month: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(year, month, day).unwrap()
}

fn id(numbering: &Numbering, on: NaiveDate, sequence: i64) -> String {
    numbering.id(on, sequence).to_string()
}

#[test]
fn numbering_makes_prefix_separator_date_part_and_padded_sequence() {
    let on = day(2026, 10, 22);
    let numbering = |prefix, separator, middle, padding| Numbering::new(prefix, separator, middle, padding).unwrap();
    assert_eq!(id(&numbering("INV", "-", Middle::None, 4), on, 1), "INV-0001");
    assert_eq!(id(&numbering("INV", "-", Middle::Year, 4), on, 1), "INV-2026-0001");
    assert_eq!(id(&numbering("INV", "-", Middle::YearMonth, 4), on, 12), "INV-202610-0012");
    assert_eq!(id(&numbering("ACME", "-", Middle::Date, 3), on, 7), "ACME-20261022-007");
    assert_eq!(id(&numbering("RCT", "/", Middle::Date, 2), on, 7), "RCT/20261022/07");
    assert_eq!(id(&numbering("INV", "-", Middle::None, 0), on, 7), "INV-7");
    assert_eq!(id(&numbering("INV", "-", Middle::None, 2), on, 12345), "INV-12345", "a longer sequence is not cut");
    assert_eq!(id(&numbering("", "", Middle::None, 4), on, 1), "0001", "no prefix, no separator");
    assert_eq!(id(&numbering("INV", "", Middle::Year, 4), day(2027, 1, 2), 1), "INV20270001");
}

#[test]
fn the_scope_is_the_date_part() {
    let on = day(2026, 3, 9);
    let scope = |middle| Numbering::new("INV", "-", middle, 4).unwrap().scope(on);
    assert_eq!(scope(Middle::None), "");
    assert_eq!(scope(Middle::Year), "2026");
    assert_eq!(scope(Middle::YearMonth), "202603");
    assert_eq!(scope(Middle::Date), "20260309");
}

#[test]
fn a_format_that_cannot_hold_a_sequence_in_a_name_is_refused() {
    assert!(matches!(Numbering::new(&"A".repeat(300), "-", Middle::None, 4), Err(NumberingError::TooLong)));
    assert!(matches!(Numbering::new(&"A".repeat(250), "-", Middle::Date, 10), Err(NumberingError::TooLong)));
    assert!(Numbering::new(&"A".repeat(200), "-", Middle::Date, 10).is_ok());
}

/// Any function will do: a counter per month and the tenant's own words.
struct Monthly(&'static str);

impl Ids for Monthly {
    fn scope(&self, on: NaiveDate) -> String {
        on.format("%Y%m").to_string()
    }

    fn id(&self, on: NaiveDate, sequence: i64) -> Name {
        Name::try_new(format!("{}/{}/{sequence}", self.0, on.format("%m-%y"))).unwrap()
    }
}

async fn tenant(pool: &PgPool) -> Uuid {
    sqlx::query_scalar("INSERT INTO tenants (name) VALUES ('Clinic') RETURNING id").fetch_one(pool).await.unwrap()
}

async fn next(pool: &PgPool, tenant: Uuid, document: Numbered, on: NaiveDate, ids: &impl Ids) -> String {
    let mut connection = pool.acquire().await.unwrap();
    issue(&mut connection, tenant, document, on, ids).await.unwrap().to_string()
}

#[sqlx::test(migrations = false)]
async fn ids_count_up_per_tenant_kind_and_scope(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let (mine, other) = (tenant(&pool).await, tenant(&pool).await);
    let yearly = Numbering::new("INV", "-", Middle::Year, 4).unwrap();
    let (october, next_year) = (day(2026, 10, 22), day(2027, 1, 3));

    let mut made = vec![];
    for _ in 0..3 {
        made.push(next(&pool, mine, Numbered::Invoice, october, &yearly).await);
    }
    assert_eq!(made, ["INV-2026-0001", "INV-2026-0002", "INV-2026-0003"]);
    assert_eq!(next(&pool, mine, Numbered::Payment, october, &yearly).await, "INV-2026-0001", "a payment has its own count");
    assert_eq!(next(&pool, other, Numbered::Invoice, october, &yearly).await, "INV-2026-0001", "so has another tenant");
    assert_eq!(next(&pool, mine, Numbered::Invoice, next_year, &yearly).await, "INV-2027-0001", "a new year starts again");
    assert_eq!(next(&pool, mine, Numbered::Invoice, october, &yearly).await, "INV-2026-0004", "and the old one goes on where it was");

    let endless = Numbering::new("INV", "-", Middle::None, 4).unwrap();
    assert_eq!((next(&pool, mine, Numbered::Invoice, october, &endless).await, next(&pool, mine, Numbered::Invoice, next_year, &endless).await), ("INV-0001".to_string(), "INV-0002".to_string()), "no date part, one run for ever");
}

#[sqlx::test(migrations = false)]
async fn any_function_makes_the_ids(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let mine = tenant(&pool).await;
    let ids = Monthly("clinic");
    assert_eq!(next(&pool, mine, Numbered::Invoice, day(2026, 10, 1), &ids).await, "clinic/10-26/1");
    assert_eq!(next(&pool, mine, Numbered::Invoice, day(2026, 10, 30), &ids).await, "clinic/10-26/2");
    assert_eq!(next(&pool, mine, Numbered::Invoice, day(2026, 11, 2), &ids).await, "clinic/11-26/1", "the function's own scope");
}

#[sqlx::test(migrations = false)]
async fn documents_issued_at_the_same_time_get_different_numbers(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let mine = tenant(&pool).await;
    let numbering = Numbering::new("INV", "-", Middle::None, 4).unwrap();
    let tasks: Vec<_> = (0..20)
        .map(|_| {
            let (pool, numbering) = (pool.clone(), numbering.clone());
            tokio::spawn(async move {
                let mut transaction = pool.begin().await.unwrap();
                let id = issue(&mut transaction, mine, Numbered::Invoice, day(2026, 10, 22), &numbering).await.unwrap();
                transaction.commit().await.unwrap();
                id.to_string()
            })
        })
        .collect();
    let mut made = vec![];
    for task in tasks {
        made.push(task.await.unwrap());
    }
    made.sort();
    let expected: Vec<String> = (1..=20).map(|n| format!("INV-{n:04}")).collect();
    assert_eq!(made, expected, "every number once, none skipped");
}

#[sqlx::test(migrations = false)]
async fn a_document_that_rolls_back_gives_its_number_back(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let mine = tenant(&pool).await;
    let numbering = Numbering::new("INV", "-", Middle::None, 4).unwrap();
    let on = day(2026, 10, 22);
    let mut transaction = pool.begin().await.unwrap();
    assert_eq!(issue(&mut transaction, mine, Numbered::Invoice, on, &numbering).await.unwrap().to_string(), "INV-0001");
    transaction.rollback().await.unwrap();
    assert_eq!(next(&pool, mine, Numbered::Invoice, on, &numbering).await, "INV-0001", "no gap");
}

#[sqlx::test(migrations = false)]
async fn the_counter_table_follows_the_soft_delete_rule_and_migrating_twice_keeps_it(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let mine = tenant(&pool).await;
    let numbering = Numbering::new("INV", "-", Middle::None, 4).unwrap();
    next(&pool, mine, Numbered::Invoice, day(2026, 10, 22), &numbering).await;
    megh::migrate(&pool).await.unwrap();
    assert_eq!(next(&pool, mine, Numbered::Invoice, day(2026, 10, 22), &numbering).await, "INV-0002", "a new run of the migrations keeps the count");
    let columns: Vec<String> = sqlx::query_scalar("SELECT column_name::text FROM information_schema.columns WHERE table_name = 'billing_counters' AND column_name IN ('created_at', 'updated_at', 'deleted_at')")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(columns.len(), 3, "the stamps a cascading soft delete needs");
    assert!(sqlx::query("INSERT INTO billing_counters (tenant_id, document, scope, next) VALUES ($1, 'quote', '', 2)").bind(mine).execute(&pool).await.is_err(), "only invoices and payments are numbered");
}

#[sqlx::test(migrations = false)]
async fn an_issued_id_is_a_name_that_an_invoice_can_keep(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let (mine, customer) = (tenant(&pool).await, tenant(&pool).await);
    let numbering = Numbering::new("INV", "-", Middle::Year, 4).unwrap();
    let number = next(&pool, mine, Numbered::Invoice, day(2026, 10, 22), &numbering).await;
    sqlx::query("INSERT INTO billing_invoices (tenant_id, id, customer_id, number, status, date, due_date, currency) VALUES ($1, $2, $3, $4, 'open', '2026-10-22', '2026-11-21', 'USD')")
        .bind(mine)
        .bind(Uuid::now_v7())
        .bind(customer)
        .bind(&number)
        .execute(&pool)
        .await
        .unwrap();
    let stored: String = sqlx::query_scalar("SELECT number FROM billing_invoices WHERE tenant_id = $1").bind(mine).fetch_one(&pool).await.unwrap();
    assert_eq!(stored, "INV-2026-0001");
}
