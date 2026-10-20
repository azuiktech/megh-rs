//! B4 (docs/TPD-booking.md): Provider CRUD over HTTP: names, contact columns, capacity, attributes, scoped to the path's org.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::middleware::from_fn;
use axum::{Extension, Router};
use megh::auth::rest::AppContext;
use megh::{Grant, authorizer};
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

async fn org(pool: &PgPool) -> Uuid {
    sqlx::query_scalar("INSERT INTO organizations (name) VALUES ('Clinic') RETURNING id")
        .fetch_one(pool)
        .await
        .unwrap()
}

fn app(pool: PgPool, org: Uuid) -> Router {
    let (api, _) = megh::booking::router().split_for_parts();
    let grants = vec![
        Grant::new(format!("orgs:read:{org}")),
        Grant::new("providers:*"),
    ];
    Router::new()
        .nest("/api/v1", api.layer(from_fn(authorizer)))
        .layer(Extension(grants))
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
    let response = app.clone().oneshot(request.unwrap()).await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

#[sqlx::test(migrations = false)]
async fn provider_with_contact_columns(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let (mine, other) = (org(&pool).await, org(&pool).await);
    let app = app(pool, mine);
    let collection = format!("/api/v1/orgs/{mine}/providers");

    let (status, dentist) = call(
        &app,
        "POST",
        &collection,
        json!({
            "org_id": other,
            "first_name": "Ada", "last_name": "Lovelace", "name": "Dr Lovelace",
            "email": "ada@example.com", "mobile_phone": "+1 (516) 555-0100",
            "attributes": {"role": "dentist", "years": 12}
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{dentist}");
    assert_eq!(dentist["org_id"], json!(mine), "the path's org wins");
    assert_eq!(
        dentist["mobile_phone"], "+15165550100",
        "phones are kept in E.164"
    );
    assert_eq!(
        (dentist["home_phone"].clone(), dentist["work_phone"].clone()),
        (Value::Null, Value::Null)
    );
    assert_eq!(
        (dentist["capacity"].clone(), dentist["waitlist"].clone()),
        (json!(1), json!(0))
    );

    let (_, hygienist) = call(&app, "POST", &collection, json!({
        "first_name": "Grace", "last_name": "Hopper", "name": "Grace", "attributes": {"role": "hygienist"}
    })).await;
    assert_eq!(
        hygienist["email"],
        Value::Null,
        "contact fields are optional"
    );

    for bad in [
        json!({"first_name": "A", "last_name": "B", "name": "C", "email": "not-an-email"}),
        json!({"first_name": "A", "last_name": "B", "name": "C", "home_phone": "5165550100"}),
        json!({"first_name": " ", "last_name": "B", "name": "C"}),
        json!({"last_name": "B", "name": "C"}),
    ] {
        assert_eq!(
            call(&app, "POST", &collection, bad.clone()).await.0,
            StatusCode::BAD_REQUEST,
            "{bad}"
        );
    }

    let names = |page: &Value| {
        page["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| item["name"].as_str().unwrap().to_string())
            .collect::<Vec<_>>()
    };
    let by_email = format!("{collection}?filter=email%20%3D%20%22ada%40example.com%22");
    assert_eq!(
        names(&call(&app, "GET", &by_email, Value::Null).await.1),
        ["Dr Lovelace"]
    );
    let by_role = format!("{collection}?filter=role%20%3D%20%22hygienist%22");
    assert_eq!(
        names(&call(&app, "GET", &by_role, Value::Null).await.1),
        ["Grace"]
    );

    let item = format!("{collection}/{}", dentist["id"].as_str().unwrap());
    let (status, patched) = call(
        &app,
        "PATCH",
        &item,
        json!({"mobile_phone": null, "work_phone": "+44 20 7946 0958"}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{patched}");
    assert_eq!(
        (
            patched["mobile_phone"].clone(),
            patched["work_phone"].clone()
        ),
        (Value::Null, json!("+442079460958"))
    );
    assert_eq!(
        patched["email"], "ada@example.com",
        "fields not given are kept"
    );
    assert_eq!(
        call(&app, "PATCH", &item, json!({"email": "nope"})).await.0,
        StatusCode::BAD_REQUEST
    );

    let (status, got) = call(&app, "GET", &item, Value::Null).await;
    assert_eq!(
        (status, got["work_phone"].clone()),
        (StatusCode::OK, json!("+442079460958"))
    );
    assert_eq!(
        call(&app, "DELETE", &item, Value::Null).await.0,
        StatusCode::OK
    );
    assert_eq!(
        call(&app, "GET", &item, Value::Null).await.0,
        StatusCode::NOT_FOUND
    );
}
