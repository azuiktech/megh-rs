//! B5d (docs/TPD-billing.md): invoices and invoiceLines over HTTP. An invoice is created as a draft for a customer org
//! and changed only while a draft; its lines are locked with it.

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

async fn org(pool: &PgPool, name: &str) -> Uuid {
    sqlx::query_scalar("INSERT INTO organizations (name) VALUES ($1) RETURNING id").bind(name).fetch_one(pool).await.unwrap()
}

/// An org that belongs to `seller`, as the org of a client does.
async fn customer_of(pool: &PgPool, seller: Uuid) -> Uuid {
    sqlx::query_scalar("INSERT INTO organizations (name, parent_id) VALUES ('Customer', $1) RETURNING id").bind(seller).fetch_one(pool).await.unwrap()
}

async fn service(pool: &PgPool, org: Uuid) -> Uuid {
    sqlx::query_scalar(r#"INSERT INTO services (org_id, id, name, steps) VALUES ($1, $2, 'Cleaning', '[{"duration": "PT30M"}]') RETURNING id"#)
        .bind(org)
        .bind(Uuid::now_v7())
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
    vec![format!("orgs:read:{org}"), "invoices:*".into(), "invoiceLines:*".into()]
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

fn invoice(customer: Uuid) -> Value {
    json!({"customer_id": customer, "status": "draft", "date": "2026-10-22", "due_date": "2026-11-21", "currency": "USD"})
}

fn with(mut body: Value, extra: Value) -> Value {
    body.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
    body
}

fn line(invoice: Uuid) -> Value {
    json!({"invoice_id": invoice, "position": 1, "description": "Tooth cleaning", "quantity": "1.5", "unit_price": 80000000})
}

async fn open(pool: &PgPool, id: impl ToString) {
    sqlx::query("UPDATE billing_invoices SET status = 'open' WHERE id = $1::uuid").bind(id.to_string()).execute(pool).await.unwrap();
}

#[sqlx::test(migrations = false)]
async fn an_invoice_is_created_as_a_draft_for_a_customer_org(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let (mine, other) = (org(&pool, "Clinic").await, org(&pool, "Other").await);
    let ada = customer_of(&pool, mine).await;
    let app = app(pool.clone(), &grants(mine));
    let invoices = format!("/api/v1/orgs/{mine}/invoices");

    let (status, created) = call(&app, "POST", &invoices, with(invoice(ada), json!({"org_id": other, "currency": "usd"}))).await;
    assert_eq!(status, StatusCode::OK, "{created}");
    assert_eq!((created["org_id"].clone(), created["currency"].clone(), created["number"].clone(), created["status"].clone()), (json!(mine), json!("USD"), Value::Null, json!("draft")));
    assert_eq!((created["total"].clone(), created["amount_paid"].clone(), created["balance_due"].clone(), created["comments"].clone()), (json!(0), json!(0), json!(0), json!("")));

    let address = json!({"line_1": "1 Main St", "line_2": null, "city": "Austin", "region": "TX", "postal_code": "78701", "country": "US"});
    let (status, with_address) = call(&app, "POST", &invoices, with(invoice(ada), json!({"bill_to": address}))).await;
    assert_eq!((status, with_address["bill_to"].clone()), (StatusCode::OK, address.clone()));

    for status in ["open", "void"] {
        assert_eq!(call(&app, "POST", &invoices, with(invoice(ada), json!({"status": status}))).await.0, StatusCode::BAD_REQUEST, "created as {status}");
    }
    let wrong = [
        ("a number", with(invoice(ada), json!({"number": "INV-1"}))),
        ("no customer", invoice(Uuid::now_v7())),
        ("a bad currency", with(invoice(ada), json!({"currency": "ZZZ"}))),
        ("an incomplete address", with(invoice(ada), json!({"bill_to": {"city": "Austin"}}))),
        ("an empty body", json!({})),
    ];
    for (what, body) in wrong {
        assert_eq!(call(&app, "POST", &invoices, body).await.0, StatusCode::BAD_REQUEST, "{what}");
    }
}

#[sqlx::test(migrations = false)]
async fn an_invoice_is_changed_only_while_it_is_a_draft(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let (mine, other) = (org(&pool, "Clinic").await, org(&pool, "Other").await);
    let (ada, bea) = (customer_of(&pool, mine).await, customer_of(&pool, mine).await);
    let (mine_app, other_app) = (app(pool.clone(), &grants(mine)), app(pool.clone(), &grants(other)));
    let (_, created) = call(&mine_app, "POST", &format!("/api/v1/orgs/{mine}/invoices"), invoice(ada)).await;
    let id = created["id"].as_str().unwrap().to_string();
    let item = format!("/api/v1/orgs/{mine}/invoices/{id}");

    let (status, put) = call(&mine_app, "PUT", &item, with(invoice(ada), json!({"due_date": "2026-12-01", "comments": "thanks"}))).await;
    assert_eq!((status, put["due_date"].clone(), put["comments"].clone()), (StatusCode::OK, json!("2026-12-01"), json!("thanks")), "{put}");
    let (status, patched) = call(&mine_app, "PATCH", &item, json!({"comments": "thank you"})).await;
    assert_eq!((status, patched["comments"].clone(), patched["due_date"].clone()), (StatusCode::OK, json!("thank you"), json!("2026-12-01")), "{patched}");

    assert_eq!(call(&mine_app, "PUT", &item, with(invoice(ada), json!({"status": "open"}))).await.0, StatusCode::BAD_REQUEST, "a PUT does not open it");
    assert_eq!(call(&mine_app, "PUT", &item, with(invoice(ada), json!({"number": "INV-1"}))).await.0, StatusCode::BAD_REQUEST, "a PUT does not number it");
    for (field, value) in [("status", json!("open")), ("customer_id", json!(bea)), ("number", json!("INV-1"))] {
        assert_eq!(call(&mine_app, "PATCH", &item, json!({field: value})).await.0, StatusCode::BAD_REQUEST, "patching {field}");
    }
    assert_eq!(call(&mine_app, "PATCH", &format!("/api/v1/orgs/{mine}/invoices/{}", Uuid::now_v7()), json!({"comments": "x"})).await.0, StatusCode::NOT_FOUND);
    assert_eq!(call(&other_app, "DELETE", &format!("/api/v1/orgs/{other}/invoices/{id}"), Value::Null).await.0, StatusCode::NOT_FOUND, "another org's invoice");

    open(&pool, &id).await;
    assert_eq!(call(&mine_app, "GET", &item, Value::Null).await.1["status"], "open");
    assert_eq!(call(&mine_app, "PUT", &item, invoice(ada)).await.0, StatusCode::CONFLICT);
    assert_eq!(call(&mine_app, "PATCH", &item, json!({"comments": "late"})).await.0, StatusCode::CONFLICT);
    assert_eq!(call(&mine_app, "DELETE", &item, Value::Null).await.0, StatusCode::CONFLICT);
    sqlx::query("UPDATE billing_invoices SET status = 'void' WHERE id = $1::uuid").bind(&id).execute(&pool).await.unwrap();
    assert_eq!(call(&mine_app, "DELETE", &item, Value::Null).await.0, StatusCode::CONFLICT, "a void invoice stays");
}

#[sqlx::test(migrations = false)]
async fn a_draft_invoice_is_deleted_with_its_lines(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let mine = org(&pool, "Clinic").await;
    let ada = customer_of(&pool, mine).await;
    let app = app(pool.clone(), &grants(mine));
    let (_, created) = call(&app, "POST", &format!("/api/v1/orgs/{mine}/invoices"), invoice(ada)).await;
    let id: Uuid = created["id"].as_str().unwrap().parse().unwrap();
    let (_, row) = call(&app, "POST", &format!("/api/v1/orgs/{mine}/invoiceLines"), line(id)).await;
    assert_eq!(call(&app, "DELETE", &format!("/api/v1/orgs/{mine}/invoices/{id}"), Value::Null).await.0, StatusCode::OK);
    assert_eq!(call(&app, "GET", &format!("/api/v1/orgs/{mine}/invoiceLines/{}", row["id"].as_str().unwrap()), Value::Null).await.0, StatusCode::NOT_FOUND);
}

#[sqlx::test(migrations = false)]
async fn a_line_belongs_to_a_draft_invoice_of_the_org(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let (mine, other) = (org(&pool, "Clinic").await, org(&pool, "Other").await);
    let (ada, theirs_customer) = (customer_of(&pool, mine).await, customer_of(&pool, other).await);
    let app = app(pool.clone(), &[grants(mine), grants(other)].concat());
    let new = |org: Uuid, customer: Uuid| {
        let app = app.clone();
        async move { call(&app, "POST", &format!("/api/v1/orgs/{org}/invoices"), invoice(customer)).await.1["id"].as_str().unwrap().parse::<Uuid>().unwrap() }
    };
    let (first, second, theirs) = (new(mine, ada).await, new(mine, ada).await, new(other, theirs_customer).await);
    let lines = format!("/api/v1/orgs/{mine}/invoiceLines");

    let (status, created) = call(&app, "POST", &lines, with(line(first), json!({"org_id": other, "discount": "10", "tax_ids": [Uuid::now_v7()]}))).await;
    assert_eq!(status, StatusCode::OK, "{created}");
    assert_eq!((created["org_id"].clone(), created["quantity"].clone(), created["discount"].clone(), created["total"].clone()), (json!(mine), json!("1.5"), json!("10"), json!(0)));
    let (status, with_service) = call(&app, "POST", &lines, with(line(first), json!({"position": 2, "service_id": service(&pool, mine).await}))).await;
    assert_eq!(status, StatusCode::OK, "{with_service}");

    let bad = [
        ("another org's invoice", json!({"invoice_id": theirs})),
        ("no invoice", json!({"invoice_id": Uuid::now_v7()})),
        ("another org's service", json!({"service_id": service(&pool, other).await})),
        ("a product and a service", json!({"product_id": Uuid::now_v7(), "service_id": service(&pool, mine).await})),
        ("no appointment", json!({"appointment_id": Uuid::now_v7()})),
        ("a negative quantity", json!({"quantity": "-1"})),
        ("a discount over 100", json!({"discount": "101"})),
        ("no description", json!({"description": " "})),
    ];
    for (what, extra) in bad {
        assert_eq!(call(&app, "POST", &lines, with(line(first), extra)).await.0, StatusCode::BAD_REQUEST, "{what}");
    }

    let item = format!("{lines}/{}", created["id"].as_str().unwrap());
    let (status, put) = call(&app, "PUT", &item, with(line(second), json!({"description": "Whitening"}))).await;
    assert_eq!((status, put["invoice_id"].clone(), put["description"].clone()), (StatusCode::OK, json!(second), json!("Whitening")), "{put}");
    let (status, patched) = call(&app, "PATCH", &item, json!({"quantity": "3"})).await;
    assert_eq!((status, patched["quantity"].clone(), patched["description"].clone()), (StatusCode::OK, json!("3"), json!("Whitening")), "{patched}");
    assert_eq!(call(&app, "PATCH", &item, json!({"invoice_id": first})).await.0, StatusCode::BAD_REQUEST, "a patch does not move a line");
    assert_eq!(call(&app, "PUT", &item, line(theirs)).await.0, StatusCode::BAD_REQUEST, "nor into another org's invoice");
    let on_first = call(&app, "GET", &format!("{lines}?filter=invoice_id%20%3D%20%22{first}%22"), Value::Null).await.1;
    assert_eq!(on_first["items"].as_array().unwrap().len(), 1);

    open(&pool, second).await;
    assert_eq!(call(&app, "POST", &lines, line(second)).await.0, StatusCode::CONFLICT, "a new line on an invoice that is not a draft");
    assert_eq!(call(&app, "PUT", &item, line(second)).await.0, StatusCode::CONFLICT);
    assert_eq!(call(&app, "PATCH", &item, json!({"quantity": "4"})).await.0, StatusCode::CONFLICT);
    assert_eq!(call(&app, "DELETE", &item, Value::Null).await.0, StatusCode::CONFLICT);
    assert_eq!(call(&app, "GET", &item, Value::Null).await.1["quantity"], "3", "locked lines are still read");
}

#[sqlx::test(migrations = false)]
async fn authorizer_guards_invoices_and_lines(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let (mine, other) = (org(&pool, "Clinic").await, org(&pool, "Other").await);
    let ada = customer_of(&pool, mine).await;
    for (segment, body) in [("invoices", invoice(ada)), ("invoiceLines", line(Uuid::now_v7()))] {
        let collection = format!("/api/v1/orgs/{mine}/{segment}");
        let read_only = app(pool.clone(), &[format!("orgs:read:{mine}"), format!("{segment}:read")]);
        assert_eq!(call(&read_only, "GET", &collection, Value::Null).await.0, StatusCode::OK, "{segment}");
        assert_eq!(call(&read_only, "POST", &collection, body).await.0, StatusCode::FORBIDDEN, "{segment}");
        let mine_only = app(pool.clone(), &grants(mine));
        assert_eq!(call(&mine_only, "GET", &format!("/api/v1/orgs/{other}/{segment}"), Value::Null).await.0, StatusCode::FORBIDDEN, "{segment}");
    }
}

#[test]
fn invoices_and_lines_are_documented_with_their_grants() {
    let (_, api) = megh::billing::router().split_for_parts();
    for path in ["/orgs/{org_id}/invoices", "/orgs/{org_id}/invoices/{id}", "/orgs/{org_id}/invoiceLines", "/orgs/{org_id}/invoiceLines/{id}"] {
        let item = api.paths.paths.get(path).unwrap_or_else(|| panic!("{path}"));
        for operation in [&item.get, &item.post, &item.put, &item.patch, &item.delete].into_iter().flatten() {
            assert!(operation.security.as_ref().is_some_and(|security| !security.is_empty()), "{path}");
        }
    }
    let tags: Vec<_> = api.tags.unwrap_or_default().into_iter().map(|tag| tag.name).collect();
    assert!(tags.contains(&"invoices".to_string()) && tags.contains(&"invoiceLines".to_string()));
}
