//! B5c (docs/TPD-billing.md): orders and orderLines over HTTP. An order is created as a draft and changed only while a
//! draft; its lines are locked with it; its party is a client (sales order) or vendor (purchase order) of the org.

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

fn app(pool: PgPool, grants: &[String]) -> Router {
    let (api, _) = megh::billing::router().split_for_parts();
    Router::new()
        .nest("/api/v1", api.layer(from_fn(authorizer)))
        .layer(Extension(grants.iter().map(Grant::new).collect::<Vec<_>>()))
        .with_state(AppContext { pool })
}

fn grants(org: Uuid) -> Vec<String> {
    vec![format!("orgs:read:{org}"), "orders:*".into(), "orderLines:*".into()]
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

fn sales_order(party: Uuid) -> Value {
    json!({"kind": "sales_order", "party_id": party, "status": "draft", "date": "2026-10-22", "currency": "USD"})
}

fn line(order: Uuid) -> Value {
    json!({"order_id": order, "position": 1, "description": "Gloves", "quantity": "2", "unit_price": 5000000})
}

/// What B6's `:open` will do: leave the draft.
async fn open(pool: &PgPool, order: &str) {
    sqlx::query("UPDATE billing_orders SET status = 'open' WHERE id = $1::uuid").bind(order).execute(pool).await.unwrap();
}

#[sqlx::test(migrations = false)]
async fn an_order_is_created_as_a_draft_for_a_party_of_the_org(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let (mine, other) = (org(&pool).await, org(&pool).await);
    let (ada, acme, theirs) = (client(&pool, mine).await, vendor(&pool, mine).await, client(&pool, other).await);
    let app = app(pool.clone(), &grants(mine));
    let orders = format!("/api/v1/orgs/{mine}/orders");

    let (status, order) = call(&app, "POST", &orders, json!({"org_id": other, "kind": "sales_order", "party_id": ada, "status": "draft", "date": "2026-10-22", "currency": "usd"})).await;
    assert_eq!(status, StatusCode::OK, "{order}");
    assert_eq!((order["org_id"].clone(), order["currency"].clone(), order["tax_mode"].clone()), (json!(mine), json!("USD"), json!("exclusive")));
    let (status, purchase) = call(&app, "POST", &orders, json!({"kind": "purchase_order", "party_id": acme, "status": "draft", "date": "2026-10-22", "currency": "USD"})).await;
    assert_eq!(status, StatusCode::OK, "{purchase}");

    for status in ["open", "cancelled"] {
        let body = json!({"kind": "sales_order", "party_id": ada, "status": status, "date": "2026-10-22", "currency": "USD"});
        assert_eq!(call(&app, "POST", &orders, body).await.0, StatusCode::BAD_REQUEST, "created as {status}");
    }
    let wrong_party = [
        ("a vendor on a sales order", json!({"kind": "sales_order", "party_id": acme})),
        ("a client on a purchase order", json!({"kind": "purchase_order", "party_id": ada})),
        ("another org's client", json!({"kind": "sales_order", "party_id": theirs})),
        ("nobody", json!({"kind": "sales_order", "party_id": Uuid::now_v7()})),
    ];
    for (what, mut body) in wrong_party {
        body["status"] = json!("draft");
        body["date"] = json!("2026-10-22");
        body["currency"] = json!("USD");
        assert_eq!(call(&app, "POST", &orders, body).await.0, StatusCode::BAD_REQUEST, "{what}");
    }
    sqlx::query("UPDATE clients SET deleted_at = now() WHERE id = $1").bind(ada).execute(&pool).await.unwrap();
    assert_eq!(call(&app, "POST", &orders, sales_order(ada)).await.0, StatusCode::BAD_REQUEST, "a deleted client");

    for bad in [json!({"kind": "sales_order", "party_id": acme, "status": "draft", "date": "2026-10-22", "currency": "ZZZ"}), json!({})] {
        assert_eq!(call(&app, "POST", &orders, bad).await.0, StatusCode::BAD_REQUEST);
    }
}

#[sqlx::test(migrations = false)]
async fn an_order_is_changed_only_while_it_is_a_draft(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let (mine, other) = (org(&pool).await, org(&pool).await);
    let (ada, acme) = (client(&pool, mine).await, vendor(&pool, mine).await);
    let (mine_app, other_app) = (app(pool.clone(), &grants(mine)), app(pool.clone(), &grants(other)));
    let (_, order) = call(&mine_app, "POST", &format!("/api/v1/orgs/{mine}/orders"), sales_order(ada)).await;
    let id = order["id"].as_str().unwrap().to_string();
    let item = format!("/api/v1/orgs/{mine}/orders/{id}");

    let (status, put) = call(&mine_app, "PUT", &item, json!({"kind": "sales_order", "party_id": ada, "status": "draft", "date": "2026-10-23", "currency": "EUR", "reference": "PO-7"})).await;
    assert_eq!((status, put["currency"].clone(), put["reference"].clone()), (StatusCode::OK, json!("EUR"), json!("PO-7")), "{put}");
    let (status, patched) = call(&mine_app, "PATCH", &item, json!({"notes": "call first"})).await;
    assert_eq!((status, patched["notes"].clone(), patched["reference"].clone()), (StatusCode::OK, json!("call first"), json!("PO-7")), "{patched}");

    let put_with = |extra: Value| {
        let mut body = json!({"kind": "sales_order", "party_id": ada, "status": "draft", "date": "2026-10-23", "currency": "USD"});
        body.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        body
    };
    assert_eq!(call(&mine_app, "PUT", &item, put_with(json!({"status": "open"}))).await.0, StatusCode::BAD_REQUEST, "a PUT does not open it");
    assert_eq!(call(&mine_app, "PUT", &item, put_with(json!({"party_id": acme}))).await.0, StatusCode::BAD_REQUEST, "a vendor on a sales order");
    for (field, value) in [("kind", json!("purchase_order")), ("party_id", json!(acme)), ("status", json!("open"))] {
        assert_eq!(call(&mine_app, "PATCH", &item, json!({field: value})).await.0, StatusCode::BAD_REQUEST, "patching {field}");
    }
    assert_eq!(call(&mine_app, "PATCH", &format!("/api/v1/orgs/{mine}/orders/{}", Uuid::now_v7()), json!({"notes": "x"})).await.0, StatusCode::NOT_FOUND);
    assert_eq!(call(&other_app, "DELETE", &format!("/api/v1/orgs/{other}/orders/{id}"), Value::Null).await.0, StatusCode::NOT_FOUND, "another org's order");

    open(&pool, &id).await;
    assert_eq!(call(&mine_app, "GET", &item, Value::Null).await.1["status"], "open");
    assert_eq!(call(&mine_app, "PUT", &item, put_with(json!({}))).await.0, StatusCode::CONFLICT);
    assert_eq!(call(&mine_app, "PATCH", &item, json!({"notes": "late"})).await.0, StatusCode::CONFLICT);
    assert_eq!(call(&mine_app, "DELETE", &item, Value::Null).await.0, StatusCode::CONFLICT);
}

#[sqlx::test(migrations = false)]
async fn a_draft_order_can_be_deleted_with_its_lines(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let mine = org(&pool).await;
    let app = app(pool.clone(), &grants(mine));
    let (_, order) = call(&app, "POST", &format!("/api/v1/orgs/{mine}/orders"), sales_order(client(&pool, mine).await)).await;
    let order_id = order["id"].as_str().unwrap();
    let (_, row) = call(&app, "POST", &format!("/api/v1/orgs/{mine}/orderLines"), line(order_id.parse().unwrap())).await;
    let row_item = format!("/api/v1/orgs/{mine}/orderLines/{}", row["id"].as_str().unwrap());

    assert_eq!(call(&app, "DELETE", &format!("/api/v1/orgs/{mine}/orders/{order_id}"), Value::Null).await.0, StatusCode::OK);
    assert_eq!(call(&app, "GET", &row_item, Value::Null).await.0, StatusCode::NOT_FOUND, "the line goes with its order");
}

#[sqlx::test(migrations = false)]
async fn a_line_belongs_to_a_draft_order_of_the_org(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let (mine, other) = (org(&pool).await, org(&pool).await);
    let app = app(pool.clone(), &[grants(mine), grants(other)].concat());
    let party = client(&pool, mine).await;
    let (_, order) = call(&app, "POST", &format!("/api/v1/orgs/{mine}/orders"), sales_order(party)).await;
    let (_, second) = call(&app, "POST", &format!("/api/v1/orgs/{mine}/orders"), sales_order(party)).await;
    let (_, theirs) = call(&app, "POST", &format!("/api/v1/orgs/{other}/orders"), sales_order(client(&pool, other).await)).await;
    let (order_id, second_id, theirs_id): (Uuid, Uuid, Uuid) = (order["id"].as_str().unwrap().parse().unwrap(), second["id"].as_str().unwrap().parse().unwrap(), theirs["id"].as_str().unwrap().parse().unwrap());
    let lines = format!("/api/v1/orgs/{mine}/orderLines");

    let (status, created) = call(&app, "POST", &lines, json!({"org_id": other, "order_id": order_id, "position": 1, "description": "Gloves", "quantity": "2", "unit_price": 5000000, "discount": "10", "unit": "box"})).await;
    assert_eq!(status, StatusCode::OK, "{created}");
    assert_eq!((created["org_id"].clone(), created["quantity"].clone(), created["discount"].clone(), created["sub_total"].clone()), (json!(mine), json!("2"), json!("10"), json!(0)));
    let (status, with_service) = call(&app, "POST", &lines, json!({"order_id": order_id, "position": 2, "description": "Cleaning", "quantity": "1", "unit_price": 80000000, "service_id": service(&pool, mine).await})).await;
    assert_eq!(status, StatusCode::OK, "{with_service}");

    let bad = [
        ("another org's order", json!({"order_id": theirs_id})),
        ("no order", json!({"order_id": Uuid::now_v7()})),
        ("another org's service", json!({"order_id": order_id, "service_id": service(&pool, other).await})),
        ("a product and a service", json!({"order_id": order_id, "product_id": Uuid::now_v7(), "service_id": service(&pool, mine).await})),
        ("a negative quantity", json!({"order_id": order_id, "quantity": "-1"})),
        ("a discount over 100", json!({"order_id": order_id, "discount": "101"})),
        ("no description", json!({"order_id": order_id, "description": " "})),
    ];
    for (what, extra) in bad {
        let mut body = line(order_id);
        body.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        assert_eq!(call(&app, "POST", &lines, body).await.0, StatusCode::BAD_REQUEST, "{what}");
    }

    let item = format!("{lines}/{}", created["id"].as_str().unwrap());
    let (status, put) = call(&app, "PUT", &item, json!({"order_id": second_id, "position": 3, "description": "Gloves L", "quantity": "5", "unit_price": 6000000})).await;
    assert_eq!((status, put["order_id"].clone(), put["description"].clone()), (StatusCode::OK, json!(second_id), json!("Gloves L")), "{put}");
    let (status, patched) = call(&app, "PATCH", &item, json!({"quantity": "6"})).await;
    assert_eq!((status, patched["quantity"].clone(), patched["description"].clone()), (StatusCode::OK, json!("6"), json!("Gloves L")), "{patched}");
    assert_eq!(call(&app, "PATCH", &item, json!({"order_id": order_id})).await.0, StatusCode::BAD_REQUEST, "a patch does not move a line");
    assert_eq!(call(&app, "PUT", &item, line(theirs_id)).await.0, StatusCode::BAD_REQUEST, "nor into another org's order");

    let filtered = call(&app, "GET", &format!("{lines}?filter=order_id%20%3D%20%22{order_id}%22"), Value::Null).await.1;
    assert_eq!(filtered["items"].as_array().unwrap().len(), 1, "one line is left on the first order");

    sqlx::query("UPDATE billing_orders SET status = 'open' WHERE id = $1").bind(second_id).execute(&pool).await.unwrap();
    assert_eq!(call(&app, "POST", &lines, line(second_id)).await.0, StatusCode::CONFLICT, "a new line on an order that is not a draft");
    assert_eq!(call(&app, "PUT", &item, line(second_id)).await.0, StatusCode::CONFLICT);
    assert_eq!(call(&app, "PATCH", &item, json!({"quantity": "7"})).await.0, StatusCode::CONFLICT);
    assert_eq!(call(&app, "DELETE", &item, Value::Null).await.0, StatusCode::CONFLICT);
    assert_eq!(call(&app, "GET", &item, Value::Null).await.1["quantity"], "6", "locked lines are still read");
}

#[sqlx::test(migrations = false)]
async fn authorizer_guards_orders_and_lines(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let (mine, other) = (org(&pool).await, org(&pool).await);
    let party = client(&pool, mine).await;
    for (segment, body) in [("orders", sales_order(party)), ("orderLines", line(Uuid::now_v7()))] {
        let collection = format!("/api/v1/orgs/{mine}/{segment}");
        let read_only = app(pool.clone(), &[format!("orgs:read:{mine}"), format!("{segment}:read")]);
        assert_eq!(call(&read_only, "GET", &collection, Value::Null).await.0, StatusCode::OK, "{segment}");
        assert_eq!(call(&read_only, "POST", &collection, body).await.0, StatusCode::FORBIDDEN, "{segment}");
        let mine_only = app(pool.clone(), &grants(mine));
        assert_eq!(call(&mine_only, "GET", &format!("/api/v1/orgs/{other}/{segment}"), Value::Null).await.0, StatusCode::FORBIDDEN, "{segment}");
    }
}

#[test]
fn orders_and_lines_are_documented_with_their_grants() {
    let (_, api) = megh::billing::router().split_for_parts();
    for path in ["/orgs/{org_id}/orders", "/orgs/{org_id}/orders/{id}", "/orgs/{org_id}/orderLines", "/orgs/{org_id}/orderLines/{id}"] {
        let item = api.paths.paths.get(path).unwrap_or_else(|| panic!("{path}"));
        for operation in [&item.get, &item.post, &item.put, &item.patch, &item.delete].into_iter().flatten() {
            assert!(operation.security.as_ref().is_some_and(|security| !security.is_empty()), "{path}");
        }
    }
    let tags: Vec<_> = api.tags.unwrap_or_default().into_iter().map(|tag| tag.name).collect();
    assert!(tags.contains(&"orders".to_string()) && tags.contains(&"orderLines".to_string()));
}
