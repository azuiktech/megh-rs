//! B5 (docs/TPD-booking.md): Client CRUD over HTTP: gender, optional birth date, optional complete address.

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

async fn tenant(pool: &PgPool) -> Uuid {
    sqlx::query_scalar("INSERT INTO tenants (name) VALUES ('Clinic') RETURNING id")
        .fetch_one(pool)
        .await
        .unwrap()
}

fn app(pool: PgPool, tenant: Uuid) -> Router {
    let (api, _) = megh::booking::router().split_for_parts();
    let grants = vec![
        Grant::new(format!("tenants:read:{tenant}")),
        Grant::new("clients:*"),
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
async fn client_with_gender_birth_date_and_address(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let mine = tenant(&pool).await;
    let app = app(pool, mine);
    let collection = format!("/api/v1/tenants/{mine}/clients");
    let address = json!({
        "line_1": "333 Bush Street", "line_2": null, "city": "San Francisco",
        "region": "CA", "postal_code": "94104", "country": "US"
    });

    let (status, ada) = call(&app, "POST", &collection, json!({
        "first_name": "Ada", "last_name": "Lovelace", "name": "Ada",
        "email": "ada@example.com", "gender": "female", "birth_date": "1990-12-10", "address": address
    }))
    .await;
    assert_eq!(status, StatusCode::OK, "{ada}");
    assert_eq!(
        (ada["gender"].clone(), ada["birth_date"].clone()),
        (json!("female"), json!("1990-12-10"))
    );
    assert_eq!(ada["address"], address);

    let (status, rex) = call(&app, "POST", &collection, json!({"first_name": "Rex", "last_name": "Lovelace", "name": "Rex", "attributes": {"species": "dog"}})).await;
    assert_eq!(status, StatusCode::OK, "{rex}");
    assert_eq!(
        (
            rex["gender"].clone(),
            rex["birth_date"].clone(),
            rex["address"].clone()
        ),
        (json!("unknown"), Value::Null, Value::Null)
    );

    for bad in [
        json!({"first_name": "A", "last_name": "B", "name": "C", "address": {"city": "San Francisco"}}),
        json!({"first_name": "A", "last_name": "B", "name": "C", "address": {"line_1": "1", "line_2": null, "city": "X", "region": "Y", "postal_code": "1", "country": "XX"}}),
        json!({"first_name": "A", "last_name": "B", "name": "C", "gender": "robot"}),
        json!({"first_name": "A", "last_name": "B", "name": "C", "birth_date": "10/12/1990"}),
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
    let born_before_2000 = format!("{collection}?filter=birth_date%20%3C%20%222000-01-01%22");
    assert_eq!(
        names(&call(&app, "GET", &born_before_2000, Value::Null).await.1),
        ["Ada"]
    );
    let unknown_gender = format!(
        "{collection}?filter=gender%20%3D%20%22unknown%22%20AND%20species%20%3D%20%22dog%22"
    );
    assert_eq!(
        names(&call(&app, "GET", &unknown_gender, Value::Null).await.1),
        ["Rex"]
    );

    let item = format!("{collection}/{}", ada["id"].as_str().unwrap());
    let (status, moved) = call(
        &app,
        "PATCH",
        &item,
        json!({"address": {"city": "Oakland", "postal_code": "94607"}}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{moved}");
    assert_eq!(
        (
            moved["address"]["city"].clone(),
            moved["address"]["line_1"].clone()
        ),
        (json!("Oakland"), json!("333 Bush Street")),
        "merge inside the address"
    );
    let (status, cleared) = call(
        &app,
        "PATCH",
        &item,
        json!({"address": null, "birth_date": null, "gender": null}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{cleared}");
    assert_eq!(
        (
            cleared["address"].clone(),
            cleared["birth_date"].clone(),
            cleared["gender"].clone()
        ),
        (Value::Null, Value::Null, json!("unknown"))
    );
    assert_eq!(
        call(
            &app,
            "PATCH",
            &item,
            json!({"address": {"line_1": "1 Main"}})
        )
        .await
        .0,
        StatusCode::BAD_REQUEST,
        "an address must be complete"
    );

    let (status, got) = call(&app, "GET", &item, Value::Null).await;
    assert_eq!(
        (status, got["email"].clone()),
        (StatusCode::OK, json!("ada@example.com"))
    );
}
