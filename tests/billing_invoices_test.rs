//! B5d (docs/TPD-billing.md): invoices and invoiceLines over HTTP, with the rules of orders: created as a draft and
//! changed only while a draft, lines locked with the invoice, the party a client (invoice, credit note) or vendor (bill,
//! vendor credit) of the org; and a credit note or vendor credit credits an invoice or bill of the org.

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

async fn service(pool: &PgPool, org: Uuid) -> Uuid {
    sqlx::query_scalar(r#"INSERT INTO services (org_id, id, name, steps) VALUES ($1, $2, 'Cleaning', '[{"duration": "PT30M"}]') RETURNING id"#)
        .bind(org)
        .bind(Uuid::now_v7())
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn order(pool: &PgPool, org: Uuid, party: Uuid) -> Uuid {
    sqlx::query_scalar("INSERT INTO billing_orders (org_id, id, kind, party_id, status, date, currency) VALUES ($1, $2, 'sales_order', $3, 'draft', '2026-10-22', 'USD') RETURNING id")
        .bind(org)
        .bind(Uuid::now_v7())
        .bind(party)
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

fn invoice(kind: &str, party: Uuid) -> Value {
    json!({"kind": kind, "party_id": party, "status": "draft", "date": "2026-10-22", "due_date": "2026-11-21", "currency": "USD"})
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
async fn an_invoice_is_created_as_a_draft_for_a_party_of_its_kind(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let (mine, other) = (org(&pool).await, org(&pool).await);
    let (ada, acme, theirs) = (client(&pool, mine).await, vendor(&pool, mine).await, client(&pool, other).await);
    let app = app(pool.clone(), &grants(mine));
    let invoices = format!("/api/v1/orgs/{mine}/invoices");

    let (status, created) = call(&app, "POST", &invoices, with(invoice("invoice", ada), json!({"org_id": other, "currency": "usd"}))).await;
    assert_eq!(status, StatusCode::OK, "{created}");
    assert_eq!((created["org_id"].clone(), created["currency"].clone(), created["number"].clone(), created["total"].clone()), (json!(mine), json!("USD"), Value::Null, json!(0)));
    for (kind, party) in [("credit_note", ada), ("bill", acme), ("vendor_credit", acme)] {
        let (status, row) = call(&app, "POST", &invoices, invoice(kind, party)).await;
        assert_eq!(status, StatusCode::OK, "{kind}: {row}");
    }

    for status in ["open", "void"] {
        assert_eq!(call(&app, "POST", &invoices, with(invoice("invoice", ada), json!({"status": status}))).await.0, StatusCode::BAD_REQUEST, "created as {status}");
    }
    let wrong = [
        ("a vendor on an invoice", invoice("invoice", acme)),
        ("a vendor on a credit note", invoice("credit_note", acme)),
        ("a client on a bill", invoice("bill", ada)),
        ("a client on a vendor credit", invoice("vendor_credit", ada)),
        ("another org's client", invoice("invoice", theirs)),
        ("nobody", invoice("invoice", Uuid::now_v7())),
        ("a bad currency", with(invoice("invoice", ada), json!({"currency": "ZZZ"}))),
        ("an empty body", json!({})),
        ("an order of no org", with(invoice("invoice", ada), json!({"order_id": Uuid::now_v7()}))),
    ];
    for (what, body) in wrong {
        assert_eq!(call(&app, "POST", &invoices, body).await.0, StatusCode::BAD_REQUEST, "{what}");
    }
    let mine_order = order(&pool, mine, ada).await;
    let (status, from_order) = call(&app, "POST", &invoices, with(invoice("invoice", ada), json!({"order_id": mine_order}))).await;
    assert_eq!((status, from_order["order_id"].clone()), (StatusCode::OK, json!(mine_order)));
    let theirs_order = order(&pool, other, theirs).await;
    assert_eq!(call(&app, "POST", &invoices, with(invoice("invoice", ada), json!({"order_id": theirs_order}))).await.0, StatusCode::BAD_REQUEST, "another org's order");

    sqlx::query("UPDATE clients SET deleted_at = now() WHERE id = $1").bind(ada).execute(&pool).await.unwrap();
    assert_eq!(call(&app, "POST", &invoices, invoice("invoice", ada)).await.0, StatusCode::BAD_REQUEST, "a deleted client");
}

#[sqlx::test(migrations = false)]
async fn a_credit_credits_an_invoice_or_bill_of_the_org(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let (mine, other) = (org(&pool).await, org(&pool).await);
    let (ada, acme, theirs) = (client(&pool, mine).await, vendor(&pool, mine).await, client(&pool, other).await);
    let app = app(pool.clone(), &[grants(mine), grants(other)].concat());
    let invoices = format!("/api/v1/orgs/{mine}/invoices");
    let (_, owed) = call(&app, "POST", &invoices, invoice("invoice", ada)).await;
    let (_, bill) = call(&app, "POST", &invoices, invoice("bill", acme)).await;
    let (_, their_invoice) = call(&app, "POST", &format!("/api/v1/orgs/{other}/invoices"), invoice("invoice", theirs)).await;
    let credit = |kind: &str, party: Uuid, credited: &Value| with(invoice(kind, party), json!({"credited_invoice_id": credited}));

    let (status, note) = call(&app, "POST", &invoices, credit("credit_note", ada, &owed["id"])).await;
    assert_eq!((status, note["credited_invoice_id"].clone()), (StatusCode::OK, owed["id"].clone()), "{note}");
    assert_eq!(call(&app, "POST", &invoices, credit("vendor_credit", acme, &bill["id"])).await.0, StatusCode::OK);
    assert_eq!(call(&app, "POST", &invoices, invoice("credit_note", ada)).await.0, StatusCode::OK, "a credit may be drafted before it names its invoice");

    let wrong = [
        ("a credit note of a bill", credit("credit_note", ada, &bill["id"])),
        ("a vendor credit of an invoice", credit("vendor_credit", acme, &owed["id"])),
        ("an invoice that credits", credit("invoice", ada, &owed["id"])),
        ("a bill that credits", credit("bill", acme, &bill["id"])),
        ("another org's invoice", credit("credit_note", ada, &their_invoice["id"])),
        ("no invoice", credit("credit_note", ada, &json!(Uuid::now_v7()))),
    ];
    for (what, body) in wrong {
        assert_eq!(call(&app, "POST", &invoices, body).await.0, StatusCode::BAD_REQUEST, "{what}");
    }
    let item = format!("{invoices}/{}", note["id"].as_str().unwrap());
    assert_eq!(call(&app, "PUT", &item, credit("credit_note", ada, &bill["id"])).await.0, StatusCode::BAD_REQUEST, "PUT is checked the same");
    assert_eq!(call(&app, "PATCH", &item, json!({"credited_invoice_id": bill["id"]})).await.0, StatusCode::BAD_REQUEST, "a patch does not change what is credited");

    assert_eq!(call(&app, "DELETE", &format!("{invoices}/{}", owed["id"].as_str().unwrap()), Value::Null).await.0, StatusCode::OK);
    assert_eq!(call(&app, "POST", &invoices, credit("credit_note", ada, &owed["id"])).await.0, StatusCode::BAD_REQUEST, "a deleted invoice");
}

#[sqlx::test(migrations = false)]
async fn an_invoice_is_changed_only_while_it_is_a_draft(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let (mine, other) = (org(&pool).await, org(&pool).await);
    let (ada, acme) = (client(&pool, mine).await, vendor(&pool, mine).await);
    let (mine_app, other_app) = (app(pool.clone(), &grants(mine)), app(pool.clone(), &grants(other)));
    let (_, created) = call(&mine_app, "POST", &format!("/api/v1/orgs/{mine}/invoices"), invoice("invoice", ada)).await;
    let id = created["id"].as_str().unwrap().to_string();
    let item = format!("/api/v1/orgs/{mine}/invoices/{id}");

    let (status, put) = call(&mine_app, "PUT", &item, with(invoice("invoice", ada), json!({"due_date": "2026-12-01", "reference": "PO-7"}))).await;
    assert_eq!((status, put["due_date"].clone(), put["reference"].clone()), (StatusCode::OK, json!("2026-12-01"), json!("PO-7")), "{put}");
    let (status, patched) = call(&mine_app, "PATCH", &item, json!({"notes": "thank you"})).await;
    assert_eq!((status, patched["notes"].clone(), patched["reference"].clone()), (StatusCode::OK, json!("thank you"), json!("PO-7")), "{patched}");

    assert_eq!(call(&mine_app, "PUT", &item, with(invoice("invoice", ada), json!({"status": "open"}))).await.0, StatusCode::BAD_REQUEST, "a PUT does not open it");
    assert_eq!(call(&mine_app, "PUT", &item, invoice("bill", ada)).await.0, StatusCode::BAD_REQUEST, "a client on a bill");
    for (field, value) in [("kind", json!("bill")), ("party_id", json!(acme)), ("status", json!("open"))] {
        assert_eq!(call(&mine_app, "PATCH", &item, json!({field: value})).await.0, StatusCode::BAD_REQUEST, "patching {field}");
    }
    assert_eq!(call(&mine_app, "PATCH", &format!("/api/v1/orgs/{mine}/invoices/{}", Uuid::now_v7()), json!({"notes": "x"})).await.0, StatusCode::NOT_FOUND);
    assert_eq!(call(&other_app, "DELETE", &format!("/api/v1/orgs/{other}/invoices/{id}"), Value::Null).await.0, StatusCode::NOT_FOUND, "another org's invoice");

    open(&pool, &id).await;
    assert_eq!(call(&mine_app, "GET", &item, Value::Null).await.1["status"], "open");
    assert_eq!(call(&mine_app, "PUT", &item, invoice("invoice", ada)).await.0, StatusCode::CONFLICT);
    assert_eq!(call(&mine_app, "PATCH", &item, json!({"notes": "late"})).await.0, StatusCode::CONFLICT);
    assert_eq!(call(&mine_app, "DELETE", &item, Value::Null).await.0, StatusCode::CONFLICT);
    sqlx::query("UPDATE billing_invoices SET status = 'void' WHERE id = $1::uuid").bind(&id).execute(&pool).await.unwrap();
    assert_eq!(call(&mine_app, "DELETE", &item, Value::Null).await.0, StatusCode::CONFLICT, "a void invoice stays");
}

#[sqlx::test(migrations = false)]
async fn a_draft_invoice_is_deleted_with_its_lines(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let mine = org(&pool).await;
    let app = app(pool.clone(), &grants(mine));
    let (_, created) = call(&app, "POST", &format!("/api/v1/orgs/{mine}/invoices"), invoice("invoice", client(&pool, mine).await)).await;
    let id: Uuid = created["id"].as_str().unwrap().parse().unwrap();
    let (_, row) = call(&app, "POST", &format!("/api/v1/orgs/{mine}/invoiceLines"), line(id)).await;
    assert_eq!(call(&app, "DELETE", &format!("/api/v1/orgs/{mine}/invoices/{id}"), Value::Null).await.0, StatusCode::OK);
    assert_eq!(call(&app, "GET", &format!("/api/v1/orgs/{mine}/invoiceLines/{}", row["id"].as_str().unwrap()), Value::Null).await.0, StatusCode::NOT_FOUND);
}

#[sqlx::test(migrations = false)]
async fn a_line_belongs_to_a_draft_invoice_of_the_org(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let (mine, other) = (org(&pool).await, org(&pool).await);
    let app = app(pool.clone(), &[grants(mine), grants(other)].concat());
    let ada = client(&pool, mine).await;
    let new = |org: Uuid, party: Uuid| {
        let app = app.clone();
        async move { call(&app, "POST", &format!("/api/v1/orgs/{org}/invoices"), invoice("invoice", party)).await.1["id"].as_str().unwrap().parse::<Uuid>().unwrap() }
    };
    let (first, second, theirs) = (new(mine, ada).await, new(mine, ada).await, new(other, client(&pool, other).await).await);
    let lines = format!("/api/v1/orgs/{mine}/invoiceLines");

    let (status, created) = call(&app, "POST", &lines, with(line(first), json!({"org_id": other, "discount": "10", "unit": "hour", "tax_ids": [Uuid::now_v7()]}))).await;
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
        ("no order line", json!({"order_line_id": Uuid::now_v7()})),
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
    let (mine, other) = (org(&pool).await, org(&pool).await);
    let party = client(&pool, mine).await;
    for (segment, body) in [("invoices", invoice("invoice", party)), ("invoiceLines", line(Uuid::now_v7()))] {
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
