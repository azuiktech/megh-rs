//! S6c (docs/TPD-billing.md section 7, #227): an invoice is receivable (the tenant bills a party) or payable (a bill
//! to the tenant; a PO while it is a draft). The other party is a client of the tenant or a tenant.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::middleware::from_fn;
use axum::{Extension, Router};
use megh::auth::rest::AppContext;
use megh::billing::ids::{Formats, Middle, Numbering};
use megh::{Grant, authorizer};
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::util::MapRequestLayer;
use tower::{Layer, ServiceExt};
use uuid::Uuid;

async fn tenant(pool: &PgPool, name: &str) -> Uuid {
    sqlx::query_scalar("INSERT INTO tenants (name) VALUES ($1) RETURNING id")
        .bind(name)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn client(pool: &PgPool, tenant: Uuid, name: &str, role: Option<&str>) -> Uuid {
    sqlx::query_scalar(
        "INSERT INTO clients (tenant_id, id, first_name, last_name, name, role) VALUES ($1, uuidv7(), $2, 'X', $2, $3) RETURNING id",
    )
    .bind(tenant)
    .bind(name)
    .bind(role)
    .fetch_one(pool)
    .await
    .unwrap()
}

fn app(pool: PgPool, tenant: Uuid) -> Router {
    let formats = Formats::new(
        Numbering::new("INV", "-", Middle::Year, 4).unwrap(),
        Numbering::new("PAY", "-", Middle::None, 4).unwrap(),
    );
    let (api, _) = megh::billing::router().split_for_parts();
    let grants = vec![
        Grant::new(format!("tenants:read:{tenant}")),
        Grant::new("invoices:*"),
    ];
    Router::new()
        .nest("/api/v1", api.layer(from_fn(authorizer)))
        .layer(Extension(grants))
        .layer(Extension(formats))
        .with_state(AppContext { pool })
}

async fn call(app: &Router, method: &str, uri: &str, body: Value) -> (StatusCode, Value) {
    let request = Request::builder().method(method).uri(uri);
    let request = match body {
        Value::Null => request.body(Body::empty()),
        body => request
            .header("content-type", "application/json")
            .body(Body::from(body.to_string())),
    };
    let service = MapRequestLayer::new(megh::booking::rewrite_aip_136).layer(app.clone());
    let response = service.oneshot(request.unwrap()).await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

fn draft(party_kind: &str, party: Uuid) -> Value {
    json!({"party_kind": party_kind, "party_id": party, "status": "draft", "date": "2026-10-27", "due_date": "2026-11-26", "currency": "USD"})
}

#[sqlx::test(migrations = false)]
async fn an_invoice_bills_a_client_or_a_tenant(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let mine = tenant(&pool, "Salon").await;
    let other = tenant(&pool, "Other salon").await;
    let ada = client(&pool, mine, "Ada", None).await;
    let theirs = client(&pool, other, "Bea", None).await;
    let app = app(pool, mine);
    let invoices = format!("/api/v1/tenants/{mine}/invoices");

    let (status, to_ada) = call(&app, "POST", &invoices, draft("client", ada)).await;
    assert_eq!(status, StatusCode::OK, "{to_ada}");
    assert_eq!(
        (
            to_ada["kind"].clone(),
            to_ada["party_kind"].clone(),
            to_ada["party_id"].clone(),
            to_ada["reference"].clone()
        ),
        (json!("receivable"), json!("client"), json!(ada), json!("")),
        "receivable unless told"
    );
    assert_eq!(
        to_ada["customer_id"],
        Value::Null,
        "the party replaces the customer"
    );

    let (status, to_tenant) = call(&app, "POST", &invoices, draft("tenant", other)).await;
    assert_eq!(status, StatusCode::OK, "{to_tenant}");

    for (case, body) in [
        ("another tenant's client", draft("client", theirs)),
        ("no such client", draft("client", Uuid::now_v7())),
        ("no such tenant", draft("tenant", Uuid::now_v7())),
        ("a client named as a tenant", draft("tenant", ada)),
        ("an unknown party kind", draft("supplier", ada)),
    ] {
        assert_eq!(
            call(&app, "POST", &invoices, body).await.0,
            StatusCode::BAD_REQUEST,
            "{case}"
        );
    }

    let item = format!("{invoices}/{}", to_ada["id"].as_str().unwrap());
    for (field, value) in [
        ("party_id", json!(other)),
        ("party_kind", json!("tenant")),
        ("kind", json!("payable")),
    ] {
        assert_eq!(
            call(&app, "PATCH", &item, json!({field: value})).await.0,
            StatusCode::BAD_REQUEST,
            "{field} is not patched"
        );
    }
}

#[sqlx::test(migrations = false)]
async fn a_payable_draft_is_a_purchase_order(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let mine = tenant(&pool, "Salon").await;
    let wholesaler = client(&pool, mine, "Colour Wholesale", Some("supplier")).await;
    let app = app(pool, mine);
    let invoices = format!("/api/v1/tenants/{mine}/invoices");
    let address = json!({
        "line_1": "333 Bush Street", "line_2": null, "city": "San Francisco",
        "region": "CA", "postal_code": "94104", "country": "US"
    });

    let mut order = draft("client", wholesaler);
    order["kind"] = json!("payable");
    order["reference"] = json!("PO-1001");
    order["delivery_date"] = json!("2026-11-02");
    order["deliver_to"] = address.clone();
    let (status, po) = call(&app, "POST", &invoices, order).await;
    assert_eq!(status, StatusCode::OK, "{po}");
    assert_eq!(
        (
            po["kind"].clone(),
            po["status"].clone(),
            po["reference"].clone(),
            po["delivery_date"].clone(),
            po["deliver_to"].clone()
        ),
        (
            json!("payable"),
            json!("draft"),
            json!("PO-1001"),
            json!("2026-11-02"),
            address
        )
    );

    let item = format!("{invoices}/{}", po["id"].as_str().unwrap());
    let (status, billed) = call(&app, "PATCH", &item, json!({"reference": "WH-77"})).await;
    assert_eq!(
        (status, billed["reference"].clone()),
        (StatusCode::OK, json!("WH-77")),
        "the supplier's bill number replaces the PO number while a draft"
    );
    let payables = format!("{invoices}?filter=kind%20%3D%20%22payable%22");
    let (_, page) = call(&app, "GET", &payables, Value::Null).await;
    assert_eq!(page["items"].as_array().unwrap().len(), 1, "{page}");
}

#[sqlx::test(migrations = false)]
async fn invoices_stored_before_parties_bill_their_tenant(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let mine = tenant(&pool, "Salon").await;
    let customer = tenant(&pool, "Customer").await;
    sqlx::raw_sql(
        "ALTER TABLE billing_invoices RENAME COLUMN party_id TO customer_id;
         ALTER TABLE billing_invoices DROP COLUMN party_kind, DROP COLUMN kind, DROP COLUMN reference,
             DROP COLUMN delivery_date, DROP COLUMN deliver_to",
    )
    .execute(&pool)
    .await
    .unwrap();
    let id: Uuid = sqlx::query_scalar(
        "INSERT INTO billing_invoices (tenant_id, id, customer_id, status, date, due_date, currency)
         VALUES ($1, uuidv7(), $2, 'draft', '2026-10-22', '2026-11-21', 'USD') RETURNING id",
    )
    .bind(mine)
    .bind(customer)
    .fetch_one(&pool)
    .await
    .unwrap();

    megh::migrate(&pool).await.unwrap();
    let (status, old) = call(
        &app(pool, mine),
        "GET",
        &format!("/api/v1/tenants/{mine}/invoices/{id}"),
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{old}");
    assert_eq!(
        (
            old["party_id"].clone(),
            old["party_kind"].clone(),
            old["kind"].clone(),
            old["reference"].clone()
        ),
        (
            json!(customer),
            json!("tenant"),
            json!("receivable"),
            json!("")
        )
    );
}
