//! B5e (docs/TPD-billing.md): payments and applications over HTTP. A payment is of a client (inbound) or vendor
//! (outbound); an application sets a payment (inbound for an invoice, outbound for a bill), or a credit note or vendor
//! credit, against an open invoice or bill. Refunds are designed in B6.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::middleware::from_fn;
use axum::{Extension, Router};
use megh::auth::rest::AppContext;
use megh::{authorizer, Grant};
use serde_json::{json, Value};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

async fn org(pool: &PgPool) -> Uuid {
    sqlx::query_scalar("INSERT INTO organizations (name) VALUES ('Clinic') RETURNING id").fetch_one(pool).await.unwrap()
}

async fn client(pool: &PgPool, org: Uuid) -> Uuid {
    sqlx::query_scalar("INSERT INTO clients (org_id, id, first_name, last_name, name) VALUES ($1, $2, 'Ada', 'L', 'Ada') RETURNING id")
        .bind(org)
        .bind(Uuid::now_v7())
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn vendor(pool: &PgPool, org: Uuid) -> Uuid {
    sqlx::query_scalar("INSERT INTO billing_vendors (org_id, id, name) VALUES ($1, $2, 'Acme') RETURNING id").bind(org).bind(Uuid::now_v7()).fetch_one(pool).await.unwrap()
}

/// A document written straight to the table, as B6's `:open` will leave it.
async fn document(pool: &PgPool, org: Uuid, kind: &str, party: Uuid, status: &str) -> Uuid {
    sqlx::query_scalar("INSERT INTO billing_invoices (org_id, id, kind, party_id, status, date, due_date, currency) VALUES ($1, $2, $3, $4, $5, '2026-10-22', '2026-11-21', 'USD') RETURNING id")
        .bind(org)
        .bind(Uuid::now_v7())
        .bind(kind)
        .bind(party)
        .bind(status)
        .fetch_one(pool)
        .await
        .unwrap()
}

fn app(pool: PgPool, grants: &[String]) -> Router {
    let (api, _) = megh::billing::router().split_for_parts();
    Router::new()
        .nest("/api/v1", api.layer(from_fn(authorizer)))
        .layer(Extension(grants.iter().map(Grant::new).collect::<Vec<_>>()))
        .with_state(AppContext { pool })
}

fn grants(org: Uuid) -> Vec<String> {
    vec![format!("orgs:read:{org}"), "payments:*".into(), "applications:*".into()]
}

async fn call(app: &Router, method: &str, uri: &str, body: Value) -> (StatusCode, Value) {
    let request = Request::builder().method(method).uri(uri);
    let request = match body {
        Value::Null => request.body(Body::empty()),
        body => request.header("content-type", "application/json").body(Body::from(body.to_string())),
    };
    let response = app.clone().oneshot(request.unwrap()).await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

fn payment(kind: &str, party: Uuid) -> Value {
    json!({"kind": kind, "party_id": party, "date": "2026-10-22", "amount": 120000000, "currency": "USD", "method": "card", "status": "succeeded"})
}

fn with(mut body: Value, extra: Value) -> Value {
    body.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
    body
}

async fn pay(app: &Router, org: Uuid, kind: &str, party: Uuid) -> Uuid {
    let (status, row) = call(app, "POST", &format!("/api/v1/orgs/{org}/payments"), payment(kind, party)).await;
    assert_eq!(status, StatusCode::OK, "{row}");
    row["id"].as_str().unwrap().parse().unwrap()
}

#[sqlx::test(migrations = false)]
async fn a_payment_is_of_a_party_of_its_kind(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let (mine, other) = (org(&pool).await, org(&pool).await);
    let (ada, acme, theirs) = (client(&pool, mine).await, vendor(&pool, mine).await, client(&pool, other).await);
    let app = app(pool.clone(), &grants(mine));
    let payments = format!("/api/v1/orgs/{mine}/payments");

    let (status, received) = call(&app, "POST", &payments, with(payment("inbound", ada), json!({"org_id": other, "currency": "usd"}))).await;
    assert_eq!(status, StatusCode::OK, "{received}");
    assert_eq!((received["org_id"].clone(), received["currency"].clone(), received["fees"].clone(), received["description"].clone()), (json!(mine), json!("USD"), json!(0), json!("")));
    let (status, paid) = call(&app, "POST", &payments, payment("outbound", acme)).await;
    assert_eq!(status, StatusCode::OK, "{paid}");

    let wrong = [
        ("a vendor on an inbound payment", payment("inbound", acme)),
        ("a client on an outbound payment", payment("outbound", ada)),
        ("another org's client", payment("inbound", theirs)),
        ("nobody", payment("inbound", Uuid::now_v7())),
        ("a zero amount", with(payment("inbound", ada), json!({"amount": 0}))),
        ("a negative amount", with(payment("inbound", ada), json!({"amount": -5}))),
        ("negative fees", with(payment("inbound", ada), json!({"fees": -1}))),
        ("an unknown method", with(payment("inbound", ada), json!({"method": "bitcoin"}))),
        ("an unknown status", with(payment("inbound", ada), json!({"status": "paid"}))),
        ("a bad currency", with(payment("inbound", ada), json!({"currency": "ZZZ"}))),
        ("an empty body", json!({})),
    ];
    for (what, body) in wrong {
        assert_eq!(call(&app, "POST", &payments, body).await.0, StatusCode::BAD_REQUEST, "{what}");
    }
    sqlx::query("UPDATE clients SET deleted_at = now() WHERE id = $1").bind(ada).execute(&pool).await.unwrap();
    assert_eq!(call(&app, "POST", &payments, payment("inbound", ada)).await.0, StatusCode::BAD_REQUEST, "a deleted client");
}

#[sqlx::test(migrations = false)]
async fn a_payment_is_read_replaced_patched_and_deleted(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let (mine, other) = (org(&pool).await, org(&pool).await);
    let (ada, acme) = (client(&pool, mine).await, vendor(&pool, mine).await);
    let (mine_app, other_app) = (app(pool.clone(), &grants(mine)), app(pool.clone(), &grants(other)));
    let id = pay(&mine_app, mine, "inbound", ada).await;
    let item = format!("/api/v1/orgs/{mine}/payments/{id}");

    assert_eq!(call(&mine_app, "GET", &item, Value::Null).await.1["id"], json!(id));
    let (status, put) = call(&mine_app, "PUT", &item, with(payment("inbound", ada), json!({"method": "cash", "reference": "R-1"}))).await;
    assert_eq!((status, put["method"].clone(), put["reference"].clone()), (StatusCode::OK, json!("cash"), json!("R-1")), "{put}");
    let (status, patched) = call(&mine_app, "PATCH", &item, json!({"status": "failed", "fees": 2000000})).await;
    assert_eq!((status, patched["status"].clone(), patched["fees"].clone(), patched["method"].clone()), (StatusCode::OK, json!("failed"), json!(2000000), json!("cash")), "{patched}");

    assert_eq!(call(&mine_app, "PUT", &item, payment("inbound", acme)).await.0, StatusCode::BAD_REQUEST, "a vendor on an inbound payment");
    for (field, value) in [("kind", json!("outbound")), ("party_id", json!(acme))] {
        assert_eq!(call(&mine_app, "PATCH", &item, json!({field: value})).await.0, StatusCode::BAD_REQUEST, "patching {field}");
    }
    assert_eq!(call(&mine_app, "PATCH", &item, json!({"amount": 0})).await.0, StatusCode::BAD_REQUEST, "a patch is checked too");
    assert_eq!(call(&mine_app, "GET", &format!("/api/v1/orgs/{mine}/payments/{}", Uuid::now_v7()), Value::Null).await.0, StatusCode::NOT_FOUND);
    assert_eq!(call(&other_app, "GET", &format!("/api/v1/orgs/{other}/payments/{id}"), Value::Null).await.0, StatusCode::NOT_FOUND, "another org's payment");

    let owed = document(&pool, mine, "invoice", ada, "open").await;
    let (_, applied) = call(&mine_app, "POST", &format!("/api/v1/orgs/{mine}/applications"), json!({"invoice_id": owed, "payment_id": id, "amount": 50000000})).await;
    assert_eq!(call(&mine_app, "DELETE", &item, Value::Null).await.0, StatusCode::CONFLICT, "a payment with applications stays");
    assert_eq!(call(&mine_app, "DELETE", &format!("/api/v1/orgs/{mine}/applications/{}", applied["id"].as_str().unwrap()), Value::Null).await.0, StatusCode::OK);
    assert_eq!(call(&mine_app, "DELETE", &item, Value::Null).await.0, StatusCode::OK, "once its applications are removed");
    assert_eq!(call(&mine_app, "GET", &item, Value::Null).await.0, StatusCode::NOT_FOUND);
}

#[sqlx::test(migrations = false)]
async fn a_payment_is_applied_to_an_open_invoice_or_bill_of_its_party(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let (mine, other) = (org(&pool).await, org(&pool).await);
    let (ada, bea, acme, theirs) = (client(&pool, mine).await, client(&pool, mine).await, vendor(&pool, mine).await, client(&pool, other).await);
    let app = app(pool.clone(), &[grants(mine), grants(other)].concat());
    let applications = format!("/api/v1/orgs/{mine}/applications");
    let (owed, bill) = (document(&pool, mine, "invoice", ada, "open").await, document(&pool, mine, "bill", acme, "open").await);
    let (received, paid) = (pay(&app, mine, "inbound", ada).await, pay(&app, mine, "outbound", acme).await);

    let (status, applied) = call(&app, "POST", &applications, json!({"org_id": other, "invoice_id": owed, "payment_id": received, "amount": 50000000})).await;
    assert_eq!(status, StatusCode::OK, "{applied}");
    assert_eq!((applied["org_id"].clone(), applied["amount"].clone(), applied["credit_id"].clone()), (json!(mine), json!(50000000), Value::Null));
    assert_eq!(call(&app, "POST", &applications, json!({"invoice_id": bill, "payment_id": paid, "amount": 1000000})).await.0, StatusCode::OK, "a bill is paid out");

    let (draft, void) = (document(&pool, mine, "invoice", ada, "draft").await, document(&pool, mine, "invoice", ada, "void").await);
    for (what, invoice) in [("a draft invoice", draft), ("a void invoice", void)] {
        assert_eq!(call(&app, "POST", &applications, json!({"invoice_id": invoice, "payment_id": received, "amount": 1000000})).await.0, StatusCode::CONFLICT, "{what}");
    }
    let (their_invoice, their_payment) = (document(&pool, other, "invoice", theirs, "open").await, pay(&app, other, "inbound", theirs).await);
    let wrong = [
        ("neither a payment nor a credit", json!({"invoice_id": owed, "amount": 1000000})),
        ("both a payment and a credit", json!({"invoice_id": owed, "payment_id": received, "credit_id": owed, "amount": 1000000})),
        ("a zero amount", json!({"invoice_id": owed, "payment_id": received, "amount": 0})),
        ("a negative amount", json!({"invoice_id": owed, "payment_id": received, "amount": -1})),
        ("another org's invoice", json!({"invoice_id": their_invoice, "payment_id": received, "amount": 1000000})),
        ("no invoice", json!({"invoice_id": Uuid::now_v7(), "payment_id": received, "amount": 1000000})),
        ("another org's payment", json!({"invoice_id": owed, "payment_id": their_payment, "amount": 1000000})),
        ("no payment", json!({"invoice_id": owed, "payment_id": Uuid::now_v7(), "amount": 1000000})),
        ("a payment of another client", json!({"invoice_id": document(&pool, mine, "invoice", bea, "open").await, "payment_id": received, "amount": 1000000})),
        ("an outbound payment on an invoice", json!({"invoice_id": owed, "payment_id": paid, "amount": 1000000})),
        ("an inbound payment on a bill", json!({"invoice_id": bill, "payment_id": received, "amount": 1000000})),
        ("a payment on a credit note", json!({"invoice_id": document(&pool, mine, "credit_note", ada, "open").await, "payment_id": received, "amount": 1000000})),
    ];
    for (what, body) in wrong {
        assert_eq!(call(&app, "POST", &applications, body).await.0, StatusCode::BAD_REQUEST, "{what}");
    }
}

#[sqlx::test(migrations = false)]
async fn a_credit_is_applied_to_the_invoice_or_bill_it_credits(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let mine = org(&pool).await;
    let (ada, bea, acme) = (client(&pool, mine).await, client(&pool, mine).await, vendor(&pool, mine).await);
    let app = app(pool.clone(), &grants(mine));
    let applications = format!("/api/v1/orgs/{mine}/applications");
    let (owed, bill) = (document(&pool, mine, "invoice", ada, "open").await, document(&pool, mine, "bill", acme, "open").await);
    let (note, vendor_credit) = (document(&pool, mine, "credit_note", ada, "open").await, document(&pool, mine, "vendor_credit", acme, "open").await);

    let (status, applied) = call(&app, "POST", &applications, json!({"invoice_id": owed, "credit_id": note, "amount": 10000000})).await;
    assert_eq!((status, applied["credit_id"].clone(), applied["payment_id"].clone()), (StatusCode::OK, json!(note), Value::Null), "{applied}");
    assert_eq!(call(&app, "POST", &applications, json!({"invoice_id": bill, "credit_id": vendor_credit, "amount": 10000000})).await.0, StatusCode::OK);

    let wrong = [
        ("a credit note on a bill", json!({"invoice_id": bill, "credit_id": note})),
        ("a vendor credit on an invoice", json!({"invoice_id": owed, "credit_id": vendor_credit})),
        ("an invoice as a credit", json!({"invoice_id": owed, "credit_id": document(&pool, mine, "invoice", ada, "open").await})),
        ("a credit of another client", json!({"invoice_id": owed, "credit_id": document(&pool, mine, "credit_note", bea, "open").await})),
        ("a credit that is a draft", json!({"invoice_id": owed, "credit_id": document(&pool, mine, "credit_note", ada, "draft").await})),
        ("no credit", json!({"invoice_id": owed, "credit_id": Uuid::now_v7()})),
    ];
    for (what, body) in wrong {
        let expected = if what == "a credit that is a draft" { StatusCode::CONFLICT } else { StatusCode::BAD_REQUEST };
        assert_eq!(call(&app, "POST", &applications, with(body, json!({"amount": 1000000}))).await.0, expected, "{what}");
    }
}

#[sqlx::test(migrations = false)]
async fn an_application_is_read_replaced_patched_and_removed(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let mine = org(&pool).await;
    let (ada, acme) = (client(&pool, mine).await, vendor(&pool, mine).await);
    let app = app(pool.clone(), &grants(mine));
    let applications = format!("/api/v1/orgs/{mine}/applications");
    let (first, second, bill) = (document(&pool, mine, "invoice", ada, "open").await, document(&pool, mine, "invoice", ada, "open").await, document(&pool, mine, "bill", acme, "open").await);
    let received = pay(&app, mine, "inbound", ada).await;
    let (_, created) = call(&app, "POST", &applications, json!({"invoice_id": first, "payment_id": received, "amount": 40000000})).await;
    let item = format!("{applications}/{}", created["id"].as_str().unwrap());

    let (status, patched) = call(&app, "PATCH", &item, json!({"amount": 45000000})).await;
    assert_eq!((status, patched["amount"].clone(), patched["invoice_id"].clone()), (StatusCode::OK, json!(45000000), json!(first)), "{patched}");
    for field in ["invoice_id", "payment_id", "credit_id"] {
        assert_eq!(call(&app, "PATCH", &item, json!({field: Uuid::now_v7()})).await.0, StatusCode::BAD_REQUEST, "patching {field}");
    }
    assert_eq!(call(&app, "PATCH", &item, json!({"amount": 0})).await.0, StatusCode::BAD_REQUEST);
    let (status, put) = call(&app, "PUT", &item, json!({"invoice_id": second, "payment_id": received, "amount": 5000000})).await;
    assert_eq!((status, put["invoice_id"].clone()), (StatusCode::OK, json!(second)), "{put}");
    assert_eq!(call(&app, "PUT", &item, json!({"invoice_id": bill, "payment_id": received, "amount": 5000000})).await.0, StatusCode::BAD_REQUEST, "a PUT is checked like a create");

    let on_second = call(&app, "GET", &format!("{applications}?filter=invoice_id%20%3D%20%22{second}%22"), Value::Null).await.1;
    assert_eq!(on_second["items"].as_array().unwrap().len(), 1);
    assert_eq!(call(&app, "DELETE", &item, Value::Null).await.0, StatusCode::OK);
    assert_eq!(call(&app, "GET", &item, Value::Null).await.0, StatusCode::NOT_FOUND);
}

#[sqlx::test(migrations = false)]
async fn authorizer_guards_payments_and_applications(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let (mine, other) = (org(&pool).await, org(&pool).await);
    let party = client(&pool, mine).await;
    for (segment, body) in [("payments", payment("inbound", party)), ("applications", json!({"invoice_id": Uuid::now_v7(), "payment_id": Uuid::now_v7(), "amount": 1}))] {
        let collection = format!("/api/v1/orgs/{mine}/{segment}");
        let read_only = app(pool.clone(), &[format!("orgs:read:{mine}"), format!("{segment}:read")]);
        assert_eq!(call(&read_only, "GET", &collection, Value::Null).await.0, StatusCode::OK, "{segment}");
        assert_eq!(call(&read_only, "POST", &collection, body).await.0, StatusCode::FORBIDDEN, "{segment}");
        let mine_only = app(pool.clone(), &grants(mine));
        assert_eq!(call(&mine_only, "GET", &format!("/api/v1/orgs/{other}/{segment}"), Value::Null).await.0, StatusCode::FORBIDDEN, "{segment}");
    }
}

#[test]
fn payments_and_applications_are_documented_with_their_grants() {
    let (_, api) = megh::billing::router().split_for_parts();
    for path in ["/orgs/{org_id}/payments", "/orgs/{org_id}/payments/{id}", "/orgs/{org_id}/applications", "/orgs/{org_id}/applications/{id}"] {
        let item = api.paths.paths.get(path).unwrap_or_else(|| panic!("{path}"));
        for operation in [&item.get, &item.post, &item.put, &item.patch, &item.delete].into_iter().flatten() {
            assert!(operation.security.as_ref().is_some_and(|security| !security.is_empty()), "{path}");
        }
    }
    let tags: Vec<_> = api.tags.unwrap_or_default().into_iter().map(|tag| tag.name).collect();
    assert!(tags.contains(&"payments".to_string()) && tags.contains(&"applications".to_string()));
}
