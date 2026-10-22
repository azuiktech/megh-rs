//! B5b (docs/TPD-billing.md): CRUD over HTTP for taxes, paymentTerms, series and vendors, scoped to the org in the path
//! and checked by `authorizer`, like the booking collections.

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

/// One collection and the bodies the spec uses against it.
struct Kind {
    segment: &'static str,
    minimal: Value,
    defaults: Vec<(&'static str, Value)>,
    replace: Value,
    patch: Value,
    patched: (&'static str, Value),
    bad: Vec<Value>,
}

fn kinds() -> Vec<Kind> {
    vec![
        Kind {
            segment: "taxes",
            minimal: json!({"name": "Sales tax", "rate": "7.5"}),
            defaults: vec![("description", json!("")), ("attributes", json!({}))],
            replace: json!({"name": "VAT", "rate": "20"}),
            patch: json!({"rate": "8"}),
            patched: ("rate", json!("8")),
            bad: vec![json!({"name": "T", "rate": "101"}), json!({"name": "T", "rate": "-1"}), json!({"name": " ", "rate": "5"}), json!({})],
        },
        Kind {
            segment: "paymentTerms",
            minimal: json!({"name": "Net 30", "net_days": 30}),
            defaults: vec![("description", json!(""))],
            replace: json!({"name": "Net 60", "net_days": 60}),
            patch: json!({"net_days": 45}),
            patched: ("net_days", json!(45)),
            bad: vec![json!({"name": "T", "net_days": -1}), json!({"name": "T"}), json!({"net_days": 5})],
        },
        Kind {
            segment: "series",
            minimal: json!({"name": "Invoices", "next": 1}),
            defaults: vec![("prefix", json!("")), ("padding", json!(0))],
            replace: json!({"name": "Bills", "prefix": "B-", "next": 10, "padding": 3}),
            patch: json!({"next": 7}),
            patched: ("next", json!(7)),
            bad: vec![json!({"name": "S", "next": 0}), json!({"name": "S", "next": 1, "padding": -1}), json!({"next": 1})],
        },
        Kind {
            segment: "vendors",
            minimal: json!({"name": "Acme Supplies", "email": "ap@acme.example"}),
            defaults: vec![("description", json!("")), ("attributes", json!({})), ("address", Value::Null), ("currency", Value::Null)],
            replace: json!({"name": "Acme Ltd", "currency": "EUR"}),
            patch: json!({"currency": "GBP"}),
            patched: ("currency", json!("GBP")),
            bad: vec![
                json!({"name": "V", "email": "not-an-email"}),
                json!({"name": "V", "currency": "ZZZ"}),
                json!({"name": "V", "payment_term_id": Uuid::now_v7()}),
                json!({}),
            ],
        },
    ]
}

async fn org(pool: &PgPool) -> Uuid {
    sqlx::query_scalar("INSERT INTO organizations (name) VALUES ('Clinic') RETURNING id").fetch_one(pool).await.unwrap()
}

fn app(pool: PgPool, grants: &[String]) -> Router {
    let (api, _) = megh::billing::router().split_for_parts();
    Router::new()
        .nest("/api/v1", api.layer(from_fn(authorizer)))
        .layer(Extension(grants.iter().map(Grant::new).collect::<Vec<_>>()))
        .with_state(AppContext { pool })
}

fn grants(org: Uuid) -> Vec<String> {
    ["taxes", "paymentTerms", "series", "vendors"].iter().map(|resource| format!("{resource}:*")).chain([format!("orgs:read:{org}")]).collect()
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

#[sqlx::test(migrations = false)]
async fn create_takes_the_org_from_the_path_and_fills_defaults(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let (mine, other) = (org(&pool).await, org(&pool).await);
    let app = app(pool, &grants(mine));
    for kind in kinds() {
        let collection = format!("/api/v1/orgs/{mine}/{}", kind.segment);
        let mut body = kind.minimal.clone();
        body["org_id"] = json!(other);
        let (status, row) = call(&app, "POST", &collection, body).await;
        assert_eq!(status, StatusCode::OK, "{}: {row}", kind.segment);
        assert_eq!(row["org_id"], json!(mine), "{}: the path's org wins over the body's", kind.segment);
        assert!(row["id"].is_string() && row["created_at"].is_string(), "{}", kind.segment);
        for (field, value) in &kind.defaults {
            assert_eq!(&row[*field], value, "{}.{field}", kind.segment);
        }
    }
}

#[sqlx::test(migrations = false)]
async fn invalid_values_are_a_bad_request(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let mine = org(&pool).await;
    let app = app(pool, &grants(mine));
    for kind in kinds() {
        for bad in &kind.bad {
            let (status, _) = call(&app, "POST", &format!("/api/v1/orgs/{mine}/{}", kind.segment), bad.clone()).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{}: {bad}", kind.segment);
        }
    }
}

#[sqlx::test(migrations = false)]
async fn get_put_patch_delete_one_row(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let (mine, other) = (org(&pool).await, org(&pool).await);
    let (mine_app, other_app) = (app(pool.clone(), &grants(mine)), app(pool, &grants(other)));
    for kind in kinds() {
        let collection = format!("/api/v1/orgs/{mine}/{}", kind.segment);
        let (_, row) = call(&mine_app, "POST", &collection, kind.minimal.clone()).await;
        let item = format!("{collection}/{}", row["id"].as_str().unwrap());
        let ctx = kind.segment;

        assert_eq!(call(&mine_app, "GET", &format!("{collection}/{}", Uuid::now_v7()), Value::Null).await.0, StatusCode::NOT_FOUND, "{ctx}");
        assert_eq!(call(&mine_app, "GET", &item, Value::Null).await.1["id"], row["id"], "{ctx}");

        let (status, replaced) = call(&mine_app, "PUT", &item, kind.replace.clone()).await;
        assert_eq!(status, StatusCode::OK, "{ctx}: {replaced}");
        assert_eq!(replaced["name"], kind.replace["name"], "{ctx}");

        let (status, patched) = call(&mine_app, "PATCH", &item, kind.patch.clone()).await;
        assert_eq!(status, StatusCode::OK, "{ctx}: {patched}");
        assert_eq!(patched[kind.patched.0], kind.patched.1, "{ctx}");
        assert_eq!(patched["name"], kind.replace["name"], "{ctx}: fields not given are kept");

        let theirs = format!("/api/v1/orgs/{other}/{}/{}", kind.segment, row["id"].as_str().unwrap());
        assert_eq!(call(&other_app, "GET", &theirs, Value::Null).await.0, StatusCode::NOT_FOUND, "{ctx}: another org's row is not found under mine");
        assert_eq!(call(&other_app, "DELETE", &theirs, Value::Null).await.0, StatusCode::NOT_FOUND, "{ctx}");

        let (status, deleted) = call(&mine_app, "DELETE", &item, Value::Null).await;
        assert_eq!((status, deleted["id"].clone()), (StatusCode::OK, row["id"].clone()), "{ctx}");
        assert_eq!(call(&mine_app, "GET", &item, Value::Null).await.0, StatusCode::NOT_FOUND, "{ctx}");
        assert_eq!(call(&mine_app, "DELETE", &item, Value::Null).await.0, StatusCode::NOT_FOUND, "{ctx}");
        assert_eq!(call(&mine_app, "PUT", &item, kind.replace.clone()).await.0, StatusCode::NOT_FOUND, "{ctx}: a deleted row stays deleted");
    }
}

#[sqlx::test(migrations = false)]
async fn a_vendor_names_a_payment_term_of_its_org(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let (mine, other) = (org(&pool).await, org(&pool).await);
    let app = app(pool, &[grants(mine), grants(other)].concat());
    let (_, term) = call(&app, "POST", &format!("/api/v1/orgs/{mine}/paymentTerms"), json!({"name": "Net 30", "net_days": 30})).await;
    let (_, theirs) = call(&app, "POST", &format!("/api/v1/orgs/{other}/paymentTerms"), json!({"name": "Net 10", "net_days": 10})).await;
    let vendors = format!("/api/v1/orgs/{mine}/vendors");
    let (status, vendor) = call(&app, "POST", &vendors, json!({"name": "Acme", "payment_term_id": term["id"]})).await;
    assert_eq!((status, vendor["payment_term_id"].clone()), (StatusCode::OK, term["id"].clone()));
    let (status, _) = call(&app, "POST", &vendors, json!({"name": "Acme", "payment_term_id": theirs["id"]})).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "another org's payment term");
}

#[sqlx::test(migrations = false)]
async fn list_filters_and_pages_with_a_cursor(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let mine = org(&pool).await;
    let app = app(pool, &grants(mine));
    let collection = format!("/api/v1/orgs/{mine}/taxes");
    for (name, rate) in [("Reduced", "5"), ("Standard", "20"), ("Luxury", "25"), ("Zero", "0")] {
        let (status, body) = call(&app, "POST", &collection, json!({"name": name, "rate": rate, "attributes": {"country": "FR"}})).await;
        assert_eq!(status, StatusCode::OK, "{body}");
    }
    let names = |page: &Value| page["items"].as_array().unwrap().iter().map(|item| item["name"].as_str().unwrap().to_string()).collect::<Vec<_>>();

    let (status, all) = call(&app, "GET", &collection, Value::Null).await;
    assert_eq!((status, names(&all).len(), all["next_page_token"].clone()), (StatusCode::OK, 4, Value::Null));

    let (_, filtered) = call(&app, "GET", &format!("{collection}?filter=rate%20%3E%2010%20AND%20country%20%3D%20%22FR%22"), Value::Null).await;
    assert_eq!(names(&filtered), ["Standard", "Luxury"]);

    let (_, first) = call(&app, "GET", &format!("{collection}?page_size=3"), Value::Null).await;
    let token = first["next_page_token"].as_str().expect("more pages").to_string();
    let (_, second) = call(&app, "GET", &format!("{collection}?page_size=3&page_token={token}"), Value::Null).await;
    assert_eq!((names(&first).len(), names(&second).len(), second["next_page_token"].clone()), (3, 1, Value::Null));

    assert_eq!(call(&app, "GET", &format!("{collection}?page_token=garbage"), Value::Null).await.0, StatusCode::BAD_REQUEST);
}

#[sqlx::test(migrations = false)]
async fn authorizer_guards_every_collection(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let (mine, other) = (org(&pool).await, org(&pool).await);
    for kind in kinds() {
        let collection = format!("/api/v1/orgs/{mine}/{}", kind.segment);
        let read_only = app(pool.clone(), &[format!("orgs:read:{mine}"), format!("{}:read", kind.segment)]);
        assert_eq!(call(&read_only, "GET", &collection, Value::Null).await.0, StatusCode::OK, "{}", kind.segment);
        assert_eq!(call(&read_only, "POST", &collection, kind.minimal.clone()).await.0, StatusCode::FORBIDDEN, "{}", kind.segment);
        let mine_only = app(pool.clone(), &grants(mine));
        assert_eq!(call(&mine_only, "GET", &format!("/api/v1/orgs/{other}/{}", kind.segment), Value::Null).await.0, StatusCode::FORBIDDEN, "{}", kind.segment);
    }
}

#[test]
fn every_route_is_documented_with_its_grants() {
    let (_, api) = megh::billing::router().split_for_parts();
    assert_eq!(api.info.title, "megh billing");
    for kind in kinds() {
        for path in [format!("/orgs/{{org_id}}/{}", kind.segment), format!("/orgs/{{org_id}}/{}/{{id}}", kind.segment)] {
            let item = api.paths.paths.get(&path).unwrap_or_else(|| panic!("{path}"));
            let operations = [&item.get, &item.post, &item.put, &item.patch, &item.delete];
            assert!(operations.iter().copied().flatten().count() >= 2, "{path}");
            for operation in operations.into_iter().flatten() {
                let security = operation.security.as_ref().unwrap_or_else(|| panic!("{path}: no security"));
                assert!(!security.is_empty(), "{path}");
            }
        }
    }
}
