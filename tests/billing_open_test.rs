//! S3 (docs/TPD-billing.md): `:open` and `:void` an invoice over HTTP. `:open` needs a line, takes the number from
//! the app's `Ids`, fixes the totals of the lines and the invoice, keeps `bill_to`, and the invoice is then locked;
//! `:void` is for an open invoice nothing was paid on.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::middleware::from_fn;
use axum::{Extension, Router};
use megh::auth::rest::AppContext;
use megh::billing::ids::{Formats, Middle, Numbering};
use megh::{authorizer, Grant};
use serde_json::{json, Value};
use sqlx::PgPool;
use tower::util::MapRequestLayer;
use tower::{Layer, ServiceExt};
use uuid::Uuid;

async fn org(pool: &PgPool, name: &str) -> Uuid {
    sqlx::query_scalar("INSERT INTO organizations (name) VALUES ($1) RETURNING id").bind(name).fetch_one(pool).await.unwrap()
}

fn formats() -> Formats {
    Formats::new(Numbering::new("INV", "-", Middle::Year, 4).unwrap(), Numbering::new("PAY", "-", Middle::None, 4).unwrap())
}

fn app_with(pool: PgPool, grants: &[String], formats: Option<Formats>) -> Router {
    let (api, _) = megh::billing::router().split_for_parts();
    let router = Router::new().nest("/api/v1", api.layer(from_fn(authorizer))).layer(Extension(grants.iter().map(Grant::new).collect::<Vec<_>>()));
    let router = match formats {
        Some(formats) => router.layer(Extension(formats)),
        None => router,
    };
    router.with_state(AppContext { pool })
}

fn app(pool: PgPool, org: Uuid) -> Router {
    app_with(pool, &grants(org), Some(formats()))
}

fn grants(org: Uuid) -> Vec<String> {
    vec![format!("orgs:read:{org}"), "invoices:*".into(), "invoiceLines:*".into(), "taxes:*".into()]
}

async fn call(app: &Router, method: &str, uri: &str, body: Value) -> (StatusCode, Value) {
    let request = Request::builder().method(method).uri(uri);
    let request = match body {
        Value::Null => request.body(Body::empty()),
        body => request.header("content-type", "application/json").body(Body::from(body.to_string())),
    };
    let service = MapRequestLayer::new(megh::booking::rewrite_aip_136).layer(app.clone());
    let response = service.oneshot(request.unwrap()).await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

fn path(org: Uuid, collection: &str) -> String {
    format!("/api/v1/orgs/{org}/{collection}")
}

async fn draft(app: &Router, org: Uuid, customer: Uuid, date: &str) -> Uuid {
    let body = json!({"customer_id": customer, "status": "draft", "date": date, "due_date": "2027-12-01", "currency": "USD"});
    let (status, created) = call(app, "POST", &path(org, "invoices"), body).await;
    assert_eq!(status, StatusCode::OK, "{created}");
    created["id"].as_str().unwrap().parse().unwrap()
}

async fn add_line(app: &Router, org: Uuid, invoice: Uuid, extra: Value) -> Value {
    let mut body = json!({"invoice_id": invoice, "position": 1, "description": "Tooth cleaning", "quantity": "1", "unit_price": 10000000});
    body.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
    let (status, created) = call(app, "POST", &path(org, "invoiceLines"), body).await;
    assert_eq!(status, StatusCode::OK, "{created}");
    created
}

async fn stored_line(app: &Router, org: Uuid, row: &Value) -> Value {
    call(app, "GET", &format!("{}/{}", path(org, "invoiceLines"), row["id"].as_str().unwrap()), Value::Null).await.1
}

async fn with_one_line(app: &Router, org: Uuid, customer: Uuid, date: &str) -> Uuid {
    let invoice = draft(app, org, customer, date).await;
    add_line(app, org, invoice, json!({})).await;
    invoice
}

async fn verb(app: &Router, org: Uuid, invoice: Uuid, verb: &str) -> (StatusCode, Value) {
    call(app, "POST", &format!("/api/v1/orgs/{org}/invoices/{invoice}:{verb}"), json!({})).await
}

#[sqlx::test(migrations = false)]
async fn opening_numbers_the_invoice_and_fixes_the_totals(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let (mine, customer) = (org(&pool, "Clinic").await, org(&pool, "Ada").await);
    let app = app(pool.clone(), mine);
    let (_, tax) = call(&app, "POST", &path(mine, "taxes"), json!({"name": "Sales tax", "rate": "7.5"})).await;
    let address = json!({"line_1": "1 Main St", "line_2": null, "city": "Austin", "region": "TX", "postal_code": "78701", "country": "US"});
    let (_, created) = call(&app, "POST", &path(mine, "invoices"), json!({"customer_id": customer, "status": "draft", "date": "2026-10-22", "due_date": "2026-11-21", "currency": "USD", "bill_to": address, "comments": "thanks"})).await;
    let invoice: Uuid = created["id"].as_str().unwrap().parse().unwrap();
    let first = add_line(&app, mine, invoice, json!({"quantity": "3", "unit_price": 9990000, "discount": "10", "tax_ids": [tax["id"]]})).await;
    let second = add_line(&app, mine, invoice, json!({"position": 2, "description": "Whitening", "unit_price": 80000000})).await;

    let (status, opened) = verb(&app, mine, invoice, "open").await;
    assert_eq!(status, StatusCode::OK, "{opened}");
    assert_eq!(opened["status"], "open");
    assert_eq!(opened["number"], "INV-2026-0001", "the scope is the year of the invoice date");
    assert_eq!((opened["sub_total"].clone(), opened["tax_total"].clone(), opened["total"].clone()), (json!(106970000), json!(2020000), json!(108990000)));
    assert_eq!((opened["amount_paid"].clone(), opened["balance_due"].clone()), (json!(0), json!(108990000)));
    assert_eq!((opened["bill_to"].clone(), opened["comments"].clone(), opened["customer_id"].clone()), (address, json!("thanks"), json!(customer)), "what the draft had is kept");

    let (one, two) = (stored_line(&app, mine, &first).await, stored_line(&app, mine, &second).await);
    assert_eq!((one["sub_total"].clone(), one["total"].clone()), (json!(26970000), json!(28990000)), "the lines keep their totals");
    assert_eq!((two["sub_total"].clone(), two["total"].clone()), (json!(80000000), json!(80000000)));

    let stored = call(&app, "GET", &format!("{}/{invoice}", path(mine, "invoices")), Value::Null).await.1;
    assert_eq!(stored["number"], "INV-2026-0001");
}

#[sqlx::test(migrations = false)]
async fn numbers_follow_the_orgs_ids_and_the_invoice_date(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let (mine, other, customer) = (org(&pool, "Clinic").await, org(&pool, "Other").await, org(&pool, "Ada").await);
    let app = app_with(pool.clone(), &[grants(mine), grants(other)].concat(), Some(formats()));
    let number = |opened: (StatusCode, Value)| opened.1["number"].as_str().unwrap().to_string();

    let first = with_one_line(&app, mine, customer, "2026-10-22").await;
    let second = with_one_line(&app, mine, customer, "2026-12-31").await;
    let next_year = with_one_line(&app, mine, customer, "2027-01-03").await;
    let theirs = with_one_line(&app, other, customer, "2026-10-22").await;
    let numbers = [number(verb(&app, mine, first, "open").await), number(verb(&app, mine, second, "open").await), number(verb(&app, mine, next_year, "open").await), number(verb(&app, other, theirs, "open").await)];
    assert_eq!(numbers, ["INV-2026-0001", "INV-2026-0002", "INV-2027-0001", "INV-2026-0001"]);
}

#[sqlx::test(migrations = false)]
async fn an_invoice_that_cannot_be_opened_stays_a_draft_and_keeps_its_number_free(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let (mine, other, customer) = (org(&pool, "Clinic").await, org(&pool, "Other").await, org(&pool, "Ada").await);
    let app = app_with(pool.clone(), &[grants(mine), grants(other)].concat(), Some(formats()));

    let empty = draft(&app, mine, customer, "2026-10-22").await;
    assert_eq!(verb(&app, mine, empty, "open").await.0, StatusCode::CONFLICT, "no line");
    let unknown_tax = draft(&app, mine, customer, "2026-10-22").await;
    add_line(&app, mine, unknown_tax, json!({"tax_ids": [Uuid::now_v7()]})).await;
    assert_eq!(verb(&app, mine, unknown_tax, "open").await.0, StatusCode::BAD_REQUEST, "a tax that does not exist");
    let (_, theirs) = call(&app, "POST", &path(other, "taxes"), json!({"name": "Theirs", "rate": "5"})).await;
    let foreign_tax = draft(&app, mine, customer, "2026-10-22").await;
    add_line(&app, mine, foreign_tax, json!({"tax_ids": [theirs["id"]]})).await;
    assert_eq!(verb(&app, mine, foreign_tax, "open").await.0, StatusCode::BAD_REQUEST, "another org's tax");
    let (_, gone) = call(&app, "POST", &path(mine, "taxes"), json!({"name": "Gone", "rate": "5"})).await;
    assert_eq!(call(&app, "DELETE", &format!("{}/{}", path(mine, "taxes"), gone["id"].as_str().unwrap()), Value::Null).await.0, StatusCode::OK);
    let deleted_tax = draft(&app, mine, customer, "2026-10-22").await;
    add_line(&app, mine, deleted_tax, json!({"tax_ids": [gone["id"]]})).await;
    assert_eq!(verb(&app, mine, deleted_tax, "open").await.0, StatusCode::BAD_REQUEST, "a deleted tax");
    for invoice in [empty, unknown_tax, foreign_tax, deleted_tax] {
        let stored = call(&app, "GET", &format!("{}/{invoice}", path(mine, "invoices")), Value::Null).await.1;
        assert_eq!((stored["status"].clone(), stored["number"].clone()), (json!("draft"), Value::Null), "still a draft");
    }

    let good = with_one_line(&app, mine, customer, "2026-10-22").await;
    assert_eq!(verb(&app, mine, good, "open").await.1["number"], "INV-2026-0001", "no number was used up by the ones that failed");
    assert_eq!(verb(&app, mine, good, "open").await.0, StatusCode::CONFLICT, "already open");
    assert_eq!(verb(&app, mine, Uuid::now_v7(), "open").await.0, StatusCode::NOT_FOUND);
    let theirs_invoice = with_one_line(&app, other, customer, "2026-10-22").await;
    assert_eq!(verb(&app, mine, theirs_invoice, "open").await.0, StatusCode::NOT_FOUND, "another org's invoice");
    assert_eq!(call(&app, "POST", &format!("/api/v1/orgs/{mine}/invoices/{good}:close"), json!({})).await.0, StatusCode::BAD_REQUEST, "an unknown verb");
}

#[sqlx::test(migrations = false)]
async fn an_open_invoice_is_locked(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let (mine, customer) = (org(&pool, "Clinic").await, org(&pool, "Ada").await);
    let app = app(pool.clone(), mine);
    let invoice = with_one_line(&app, mine, customer, "2026-10-22").await;
    verb(&app, mine, invoice, "open").await;
    let item = format!("{}/{invoice}", path(mine, "invoices"));
    assert_eq!(call(&app, "PATCH", &item, json!({"comments": "late"})).await.0, StatusCode::CONFLICT);
    assert_eq!(call(&app, "DELETE", &item, Value::Null).await.0, StatusCode::CONFLICT);
    let line = json!({"invoice_id": invoice, "position": 2, "description": "More", "quantity": "1", "unit_price": 1});
    assert_eq!(call(&app, "POST", &path(mine, "invoiceLines"), line).await.0, StatusCode::CONFLICT);
}

#[sqlx::test(migrations = false)]
async fn voiding_an_open_invoice_nothing_was_paid_on(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let (mine, other, customer) = (org(&pool, "Clinic").await, org(&pool, "Other").await, org(&pool, "Ada").await);
    let app = app_with(pool.clone(), &[grants(mine), grants(other)].concat(), Some(formats()));

    let unpaid = with_one_line(&app, mine, customer, "2026-10-22").await;
    assert_eq!(verb(&app, mine, unpaid, "void").await.0, StatusCode::CONFLICT, "a draft is deleted, not voided");
    let number = verb(&app, mine, unpaid, "open").await.1["number"].clone();
    let (status, voided) = verb(&app, mine, unpaid, "void").await;
    assert_eq!((status, voided["status"].clone(), voided["number"].clone()), (StatusCode::OK, json!("void"), number), "{voided}");
    assert_eq!(verb(&app, mine, unpaid, "void").await.0, StatusCode::CONFLICT, "void twice");
    assert_eq!(verb(&app, mine, unpaid, "open").await.0, StatusCode::CONFLICT, "a void invoice is not opened");

    let paid = with_one_line(&app, mine, customer, "2026-10-22").await;
    verb(&app, mine, paid, "open").await;
    sqlx::query("UPDATE billing_invoices SET amount_paid = 1000000, balance_due = balance_due - 1000000 WHERE id = $1").bind(paid).execute(&pool).await.unwrap();
    assert_eq!(verb(&app, mine, paid, "void").await.0, StatusCode::CONFLICT, "something was paid");
    assert_eq!(verb(&app, mine, Uuid::now_v7(), "void").await.0, StatusCode::NOT_FOUND);
    let theirs = with_one_line(&app, other, customer, "2026-10-22").await;
    verb(&app, other, theirs, "open").await;
    assert_eq!(verb(&app, mine, theirs, "void").await.0, StatusCode::NOT_FOUND, "another org's invoice");
}

#[sqlx::test(migrations = false)]
async fn invoices_opened_at_the_same_time_get_different_numbers(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let (mine, customer) = (org(&pool, "Clinic").await, org(&pool, "Ada").await);
    let app = app(pool.clone(), mine);
    let mut invoices = vec![];
    for _ in 0..10 {
        invoices.push(with_one_line(&app, mine, customer, "2026-10-22").await);
    }
    let tasks: Vec<_> = invoices
        .into_iter()
        .map(|invoice| {
            let app = app.clone();
            tokio::spawn(async move { verb(&app, mine, invoice, "open").await })
        })
        .collect();
    let mut numbers = vec![];
    for task in tasks {
        let (status, opened) = task.await.unwrap();
        assert_eq!(status, StatusCode::OK, "{opened}");
        numbers.push(opened["number"].as_str().unwrap().to_string());
    }
    numbers.sort();
    let expected: Vec<String> = (1..=10).map(|n| format!("INV-2026-{n:04}")).collect();
    assert_eq!(numbers, expected, "every number once, none skipped");
}

#[sqlx::test(migrations = false)]
async fn the_app_must_give_the_router_its_ids(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let (mine, customer) = (org(&pool, "Clinic").await, org(&pool, "Ada").await);
    let without = app_with(pool.clone(), &grants(mine), None);
    let invoice = with_one_line(&without, mine, customer, "2026-10-22").await;
    assert_eq!(verb(&without, mine, invoice, "open").await.0, StatusCode::INTERNAL_SERVER_ERROR, "no Formats extension");
    let stored = call(&without, "GET", &format!("{}/{invoice}", path(mine, "invoices")), Value::Null).await.1;
    assert_eq!(stored["status"], "draft", "nothing changed");
}

#[sqlx::test(migrations = false)]
async fn the_verbs_are_guarded_and_documented(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let (mine, customer) = (org(&pool, "Clinic").await, org(&pool, "Ada").await);
    let full = app(pool.clone(), mine);
    let invoice = with_one_line(&full, mine, customer, "2026-10-22").await;
    let read_only = app_with(pool.clone(), &[format!("orgs:read:{mine}"), "invoices:read".into()], Some(formats()));
    assert_eq!(verb(&read_only, mine, invoice, "open").await.0, StatusCode::FORBIDDEN);
    assert_eq!(verb(&full, mine, invoice, "open").await.0, StatusCode::OK);

    let (_, api) = megh::billing::router().split_for_parts();
    let item = api.paths.paths.get("/orgs/{org_id}/invoices/{id}:{verb}").expect("the verbs are documented");
    assert!(item.post.as_ref().is_some_and(|operation| operation.security.as_ref().is_some_and(|security| !security.is_empty())));
}
