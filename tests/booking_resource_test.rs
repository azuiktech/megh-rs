//! B3 (docs/TPD-booking.md): Resource CRUD over HTTP, scoped to the tenant in the path and checked by `authorizer`.

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

async fn tenant(pool: &PgPool) -> Uuid {
    sqlx::query_scalar("INSERT INTO tenants (name) VALUES ('Clinic') RETURNING id").fetch_one(pool).await.unwrap()
}

fn app(pool: PgPool, grants: &[String]) -> Router {
    let (api, _) = megh::booking::router().split_for_parts();
    Router::new()
        .nest("/api/v1", api.layer(from_fn(authorizer)))
        .layer(Extension(grants.iter().map(Grant::new).collect::<Vec<_>>()))
        .with_state(AppContext { pool })
}

fn grants(tenant: Uuid) -> Vec<String> {
    vec![format!("tenants:read:{tenant}"), "resources:*".into()]
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
async fn create_takes_the_tenant_from_the_path_and_fills_defaults(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let (mine, other) = (tenant(&pool).await, tenant(&pool).await);
    let app = app(pool, &grants(mine));

    let (status, room) = call(&app, "POST", &format!("/api/v1/tenants/{mine}/resources"), json!({"tenant_id": other, "name": "  Room 1 "})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(room["tenant_id"], json!(mine), "the path's tenant wins over the body's");
    assert_eq!(room["name"], "Room 1");
    assert_eq!((room["capacity"].clone(), room["waitlist"].clone(), room["description"].clone()), (json!(1), json!(0), json!("")));
    assert_eq!(room["attributes"], json!({}));

    for bad in [json!({"name": " "}), json!({"name": "Pod", "capacity": 0}), json!({"name": "Pod", "waitlist": -1}), json!({})] {
        let (status, _) = call(&app, "POST", &format!("/api/v1/tenants/{mine}/resources"), bad.clone()).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}");
    }
}

#[sqlx::test(migrations = false)]
async fn get_put_patch_delete_one_resource(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let (mine, other) = (tenant(&pool).await, tenant(&pool).await);
    let app = app(pool.clone(), &grants(mine));
    let collection = format!("/api/v1/tenants/{mine}/resources");
    let (_, room) = call(&app, "POST", &collection, json!({"name": "Room 1", "attributes": {"floor": 1, "sea_facing": true}})).await;
    let item = format!("{collection}/{}", room["id"].as_str().unwrap());

    let (status, got) = call(&app, "GET", &item, Value::Null).await;
    assert_eq!((status, got["name"].clone()), (StatusCode::OK, json!("Room 1")));
    assert_eq!(call(&app, "GET", &format!("{collection}/{}", Uuid::now_v7()), Value::Null).await.0, StatusCode::NOT_FOUND);

    let (status, put) = call(&app, "PUT", &item, json!({"name": "Room A", "capacity": 2, "description": "Big"})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!((put["name"].clone(), put["capacity"].clone(), put["attributes"].clone()), (json!("Room A"), json!(2), json!({})));
    assert_ne!(put["updated_at"], room["updated_at"]);
    assert_eq!(put["created_at"], room["created_at"]);

    let (_, _) = call(&app, "PATCH", &item, json!({"attributes": {"floor": 3, "quiet": true}})).await;
    let (status, patched) = call(&app, "PATCH", &item, json!({"capacity": 4, "description": null, "attributes": {"floor": null}})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(patched["name"], "Room A", "fields not given are kept");
    assert_eq!(patched["capacity"], 4);
    assert_eq!(patched["description"], "", "null resets to the default");
    assert_eq!(patched["attributes"], json!({"quiet": true}), "merge goes inside attributes; null removes the key");
    assert_eq!(call(&app, "PATCH", &item, json!({"name": null})).await.0, StatusCode::BAD_REQUEST, "name has no default");
    assert_eq!(call(&app, "PATCH", &item, json!({"capacity": 0})).await.0, StatusCode::BAD_REQUEST);

    let (status, deleted) = call(&app, "DELETE", &item, Value::Null).await;
    assert_eq!(status, StatusCode::OK);
    assert!(deleted["deleted_at"].is_string());
    assert_eq!(call(&app, "GET", &item, Value::Null).await.0, StatusCode::NOT_FOUND);
    assert_eq!(call(&app, "DELETE", &item, Value::Null).await.0, StatusCode::NOT_FOUND);
    assert_eq!(call(&app, "PUT", &item, json!({"name": "Back"})).await.0, StatusCode::NOT_FOUND, "a deleted row stays deleted");

    let theirs = app_for(pool, other);
    let (_, their_room) = call(&theirs, "POST", &format!("/api/v1/tenants/{other}/resources"), json!({"name": "Theirs"})).await;
    let their_id = their_room["id"].as_str().unwrap();
    assert_eq!(call(&app, "GET", &format!("{collection}/{their_id}"), Value::Null).await.0, StatusCode::NOT_FOUND, "another tenant's row is not found under mine");
    assert_eq!(call(&app, "PUT", &format!("{collection}/{their_id}"), json!({"name": "Taken"})).await.0, StatusCode::NOT_FOUND);
    assert_eq!(call(&app, "DELETE", &format!("{collection}/{their_id}"), Value::Null).await.0, StatusCode::NOT_FOUND);
}

fn app_for(pool: PgPool, tenant: Uuid) -> Router {
    app(pool, &grants(tenant))
}

#[sqlx::test(migrations = false)]
async fn list_filters_and_pages_with_a_cursor(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let mine = tenant(&pool).await;
    let app = app(pool, &grants(mine));
    let collection = format!("/api/v1/tenants/{mine}/resources");
    for (name, capacity, floor) in [("Lobby", 10, 0), ("Room 1", 2, 1), ("Room 2", 2, 2), ("Pod", 1, 9)] {
        let (status, body) = call(&app, "POST", &collection, json!({"name": name, "capacity": capacity, "attributes": {"floor": floor}})).await;
        assert_eq!(status, StatusCode::OK, "{body}");
    }
    let names = |page: &Value| page["items"].as_array().unwrap().iter().map(|item| item["name"].as_str().unwrap().to_string()).collect::<Vec<_>>();

    let (status, all) = call(&app, "GET", &collection, Value::Null).await;
    assert_eq!((status, names(&all).len(), all["next_page_token"].clone()), (StatusCode::OK, 4, Value::Null));

    let (_, filtered) = call(&app, "GET", &format!("{collection}?filter=capacity%20%3E%201%20AND%20floor%20%3E%200"), Value::Null).await;
    assert_eq!(names(&filtered), ["Room 1", "Room 2"]);

    let (_, first) = call(&app, "GET", &format!("{collection}?page_size=3"), Value::Null).await;
    let token = first["next_page_token"].as_str().expect("more pages").to_string();
    let (_, second) = call(&app, "GET", &format!("{collection}?page_size=3&page_token={token}"), Value::Null).await;
    assert_eq!((names(&first).len(), names(&second).len(), second["next_page_token"].clone()), (3, 1, Value::Null));
    assert_eq!([names(&first), names(&second)].concat().len(), 4, "no row twice, none missed");

    assert_eq!(call(&app, "GET", &format!("{collection}?page_token=garbage"), Value::Null).await.0, StatusCode::BAD_REQUEST);
    assert_eq!(call(&app, "GET", &format!("{collection}?filter=capacity%20%3E"), Value::Null).await.0, StatusCode::BAD_REQUEST);
}

#[sqlx::test(migrations = false)]
async fn authorizer_guards_every_route(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let (mine, other) = (tenant(&pool).await, tenant(&pool).await);
    let read_only = app(pool.clone(), &[format!("tenants:read:{mine}"), "resources:read".into()]);
    assert_eq!(call(&read_only, "GET", &format!("/api/v1/tenants/{mine}/resources"), Value::Null).await.0, StatusCode::OK);
    assert_eq!(call(&read_only, "POST", &format!("/api/v1/tenants/{mine}/resources"), json!({"name": "X"})).await.0, StatusCode::FORBIDDEN);
    let mine_only = app(pool, &grants(mine));
    assert_eq!(call(&mine_only, "GET", &format!("/api/v1/tenants/{other}/resources"), Value::Null).await.0, StatusCode::FORBIDDEN);
}

#[sqlx::test(migrations = false)]
async fn the_has_filter_on_a_column_that_is_not_a_list_is_a_bad_request(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let tenant = tenant(&pool).await;
    let app = app(pool, &grants(tenant));

    let (status, body) = call(&app, "GET", &format!("/api/v1/tenants/{tenant}/resources?filter=name%3A%22Room%22"), Value::Null).await;

    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
}
