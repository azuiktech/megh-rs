//! S5 (docs/TPD-billing.md): the customer of an invoice is an org that belongs to the seller. `organizations` has a
//! `parent_id`; creating a client, by any path, also creates its org (same id, named like the client, under the
//! business that has the client); an invoice's `customer_id` must have the seller as its parent (checked on create and
//! PUT).

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::middleware::from_fn;
use axum::{Extension, Router};
use megh::auth::rest::AppContext;
use megh::billing::customer::ensure_customer;
use megh::billing::ids::{Formats, Middle, Numbering};
use megh::org::Org;
use megh::{authorizer, Grant};
use serde_json::{json, Value};
use sqlx::PgPool;
use tower::util::MapRequestLayer;
use tower::{Layer, ServiceExt};
use uuid::Uuid;

async fn org(pool: &PgPool, name: &str) -> Uuid {
    sqlx::query_scalar("INSERT INTO organizations (name, timezone) VALUES ($1, 'Asia/Kolkata') RETURNING id").bind(name).fetch_one(pool).await.unwrap()
}

async fn org_under(pool: &PgPool, name: &str, parent: Uuid) -> Uuid {
    sqlx::query_scalar("INSERT INTO organizations (name, parent_id) VALUES ($1, $2) RETURNING id").bind(name).bind(parent).fetch_one(pool).await.unwrap()
}

async fn client(pool: &PgPool, business: Uuid, name: &str) -> Uuid {
    sqlx::query_scalar("INSERT INTO clients (org_id, id, first_name, last_name, name) VALUES ($1, $2, 'Ada', 'L', $3) RETURNING id")
        .bind(business)
        .bind(Uuid::now_v7())
        .bind(name)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn stored(pool: &PgPool, id: Uuid) -> Option<Org> {
    sqlx::query_as("SELECT * FROM organizations WHERE id = $1").bind(id).fetch_optional(pool).await.unwrap()
}

#[sqlx::test(migrations = false)]
async fn an_org_may_have_a_parent_but_not_itself(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let (business, child) = (org(&pool, "Clinic").await, org(&pool, "Branch").await);
    sqlx::query("UPDATE organizations SET parent_id = $1 WHERE id = $2").bind(business).bind(child).execute(&pool).await.unwrap();
    assert_eq!(stored(&pool, child).await.unwrap().parent_id, Some(business));
    assert_eq!(stored(&pool, business).await.unwrap().parent_id, None);
    assert!(sqlx::query("UPDATE organizations SET parent_id = id WHERE id = $1").bind(child).execute(&pool).await.is_err(), "not its own parent");
    assert!(sqlx::query("UPDATE organizations SET parent_id = $1 WHERE id = $2").bind(Uuid::now_v7()).bind(child).execute(&pool).await.is_err(), "the parent is an org");
}

#[sqlx::test(migrations = false)]
async fn a_new_client_gets_an_org_with_its_own_id_under_the_business(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let business = org(&pool, "Clinic").await;
    let ada = client(&pool, business, "Ada Lovelace").await;
    let made = stored(&pool, ada).await.expect("the client's org");
    assert_eq!((made.name.as_deref(), made.parent_id, made.timezone.as_str()), (Some("Ada Lovelace"), Some(business), "Asia/Kolkata"), "named like the client, under the business, in its time zone");

    let bea = client(&pool, business, "Bea").await;
    assert_ne!(ada, bea);
    assert_eq!(stored(&pool, bea).await.unwrap().parent_id, Some(business));
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM organizations WHERE parent_id = $1").bind(business).fetch_one(&pool).await.unwrap();
    assert_eq!(count, 2);
}

#[sqlx::test(migrations = false)]
async fn a_client_created_over_http_gets_its_org_and_keeps_it_when_deleted(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let business = org(&pool, "Clinic").await;
    let (api, _) = megh::booking::router().split_for_parts();
    let grants = vec![Grant::new(format!("orgs:read:{business}")), Grant::new("clients:*")];
    let app = Router::new().nest("/api/v1", api.layer(from_fn(authorizer))).layer(Extension(grants)).with_state(AppContext { pool: pool.clone() });
    let call = |method: &'static str, uri: String, body: Value| {
        let app = app.clone();
        async move {
            let request = Request::builder().method(method).uri(uri).header("content-type", "application/json");
            let response = app.oneshot(request.body(Body::from(body.to_string())).unwrap()).await.unwrap();
            let status = response.status();
            let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
            (status, serde_json::from_slice::<Value>(&bytes).unwrap_or(Value::Null))
        }
    };
    let (status, created) = call("POST", format!("/api/v1/orgs/{business}/clients"), json!({"first_name": "Ada", "last_name": "Lovelace", "name": "Ada"})).await;
    assert_eq!(status, StatusCode::OK, "{created}");
    let id: Uuid = created["id"].as_str().unwrap().parse().unwrap();
    assert_eq!(stored(&pool, id).await.unwrap().parent_id, Some(business));
    let (status, _) = call("DELETE", format!("/api/v1/orgs/{business}/clients/{id}"), json!({})).await;
    assert_eq!(status, StatusCode::OK);
    assert!(stored(&pool, id).await.is_some(), "invoices still point to the org of a deleted client");
}

#[sqlx::test(migrations = false)]
async fn clients_without_an_org_are_given_one_once(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let business = org(&pool, "Clinic").await;
    let (old, new) = (client(&pool, business, "Old").await, client(&pool, business, "New").await);
    sqlx::query("DELETE FROM organizations WHERE id = $1").bind(old).execute(&pool).await.unwrap();
    assert!(stored(&pool, old).await.is_none(), "a client from before the trigger has no org");
    megh::migrate(&pool).await.unwrap();
    megh::migrate(&pool).await.unwrap();
    let backfilled = stored(&pool, old).await.expect("the backfill");
    assert_eq!((backfilled.name.as_deref(), backfilled.parent_id), (Some("Old"), Some(business)));
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM organizations WHERE id = ANY($1)").bind(vec![old, new]).fetch_one(&pool).await.unwrap();
    assert_eq!(count, 2, "no org twice");
}

#[sqlx::test(migrations = false)]
async fn a_customer_is_an_org_of_the_seller(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let (seller, other) = (org(&pool, "Clinic").await, org(&pool, "Other").await);
    let (ada, vendor, theirs) = (client(&pool, seller, "Ada").await, org_under(&pool, "Acme", seller).await, client(&pool, other, "Cy").await);
    assert!(ensure_customer(&pool, seller, ada).await.is_ok(), "a client");
    assert!(ensure_customer(&pool, seller, vendor).await.is_ok(), "any org under the seller, a vendor say");
    for (what, customer) in [("the seller itself", seller), ("an org with no parent", other), ("another business's client", theirs), ("nobody", Uuid::now_v7())] {
        assert!(ensure_customer(&pool, seller, customer).await.is_err(), "{what}");
    }
}

fn app(pool: PgPool, seller: Uuid) -> Router {
    let formats = Formats::new(Numbering::new("INV", "-", Middle::None, 4).unwrap(), Numbering::new("PAY", "-", Middle::None, 4).unwrap());
    let (api, _) = megh::billing::router().split_for_parts();
    let grants = vec![format!("orgs:read:{seller}"), "invoices:*".to_string(), "invoiceLines:*".to_string()];
    Router::new().nest("/api/v1", api.layer(from_fn(authorizer))).layer(Extension(grants.iter().map(Grant::new).collect::<Vec<_>>())).layer(Extension(formats)).with_state(AppContext { pool })
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

fn invoice(customer: Uuid) -> Value {
    json!({"customer_id": customer, "status": "draft", "date": "2026-10-22", "due_date": "2026-11-21", "currency": "USD"})
}

#[sqlx::test(migrations = false)]
async fn an_invoice_is_only_for_a_customer_of_the_seller(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let (seller, other) = (org(&pool, "Clinic").await, org(&pool, "Other").await);
    let (ada, vendor, theirs) = (client(&pool, seller, "Ada").await, org_under(&pool, "Acme", seller).await, client(&pool, other, "Cy").await);
    let app = app(pool.clone(), seller);
    let invoices = format!("/api/v1/orgs/{seller}/invoices");

    assert_eq!(call(&app, "POST", &invoices, invoice(ada)).await.0, StatusCode::OK, "a client");
    let (status, created) = call(&app, "POST", &invoices, invoice(vendor)).await;
    assert_eq!(status, StatusCode::OK, "an org under the seller: {created}");
    for (what, customer) in [("the seller itself", seller), ("an org with no parent", other), ("another business's client", theirs), ("nobody", Uuid::now_v7())] {
        assert_eq!(call(&app, "POST", &invoices, invoice(customer)).await.0, StatusCode::BAD_REQUEST, "{what}");
    }

    let item = format!("{invoices}/{}", created["id"].as_str().unwrap());
    assert_eq!(call(&app, "PUT", &item, invoice(ada)).await.0, StatusCode::OK, "a PUT may name another customer of the seller");
    assert_eq!(call(&app, "PUT", &item, invoice(theirs)).await.0, StatusCode::BAD_REQUEST, "but not anyone's");
    assert_eq!(call(&app, "PATCH", &item, json!({"customer_id": vendor})).await.0, StatusCode::BAD_REQUEST, "and a PATCH does not change it");
}

#[sqlx::test(migrations = false)]
async fn the_customer_is_not_checked_again_when_the_invoice_is_opened(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let (seller, other) = (org(&pool, "Clinic").await, org(&pool, "Other").await);
    let vendor = org_under(&pool, "Acme", seller).await;
    let app = app(pool.clone(), seller);
    let (_, created) = call(&app, "POST", &format!("/api/v1/orgs/{seller}/invoices"), invoice(vendor)).await;
    let id = created["id"].as_str().unwrap();
    let line = json!({"invoice_id": id, "position": 1, "description": "Work", "quantity": "1", "unit_price": 10000000});
    assert_eq!(call(&app, "POST", &format!("/api/v1/orgs/{seller}/invoiceLines"), line).await.0, StatusCode::OK);
    sqlx::query("UPDATE organizations SET parent_id = $1 WHERE id = $2").bind(other).bind(vendor).execute(&pool).await.unwrap();
    assert_eq!(call(&app, "POST", &format!("/api/v1/orgs/{seller}/invoices/{id}:open"), json!({})).await.0, StatusCode::OK, "create and PUT only");
}
