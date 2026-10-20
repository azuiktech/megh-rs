//! B6 (docs/TPD-booking.md): Service CRUD over HTTP: setup, steps with provider groups, cleanup, ISO 8601 durations.

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
        Grant::new("services:*"),
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
async fn service_with_setup_steps_and_cleanup(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let mine = org(&pool).await;
    let app = app(pool, mine);
    let collection = format!("/api/v1/orgs/{mine}/services");

    let extraction = json!({
        "name": "Tooth extraction", "setup": "PT10M", "cleanup": "PT10M",
        "steps": [
            {"duration": "PT15M", "provider1": "hygienist", "provider2": null},
            {"duration": "PT45M", "provider1": "hygienist", "provider2": "dentist"},
            {"duration": "PT15M", "provider1": "hygienist", "provider2": null}
        ],
        "attributes": {"category": "surgery"}
    });
    let (status, saved) = call(&app, "POST", &collection, extraction.clone()).await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    assert_eq!(saved["steps"], extraction["steps"]);
    assert_eq!(
        (saved["setup"].clone(), saved["cleanup"].clone()),
        (json!("PT10M"), json!("PT10M"))
    );

    let (status, colour) = call(&app, "POST", &collection, json!({
        "name": "Hair colour",
        "steps": [{"duration": "PT30M", "provider1": "stylist"}, {"duration": "PT30M"}, {"duration": "PT20M", "provider1": "stylist"}]
    }))
    .await;
    assert_eq!(status, StatusCode::OK, "{colour}");
    assert_eq!(
        (colour["setup"].clone(), colour["cleanup"].clone()),
        (json!("P0D"), json!("P0D")),
        "no setup or cleanup needed: zero, as iso8601 writes it"
    );
    assert_eq!(
        colour["steps"][1],
        json!({"duration": "PT30M", "provider1": null, "provider2": null}),
        "the client waits alone"
    );

    for bad in [
        json!({"name": "X", "steps": []}),
        json!({"name": "X"}),
        json!({"name": "X", "steps": [{"duration": "45 minutes"}]}),
        json!({"name": "X", "setup": "ten", "steps": [{"duration": "PT5M"}]}),
        json!({"name": "X", "steps": [{"duration": "PT5M", "provider1": ""}]}),
        json!({"name": "", "steps": [{"duration": "PT5M"}]}),
    ] {
        assert_eq!(
            call(&app, "POST", &collection, bad.clone()).await.0,
            StatusCode::BAD_REQUEST,
            "{bad}"
        );
    }

    let surgery = format!("{collection}?filter=category%20%3D%20%22surgery%22");
    let (_, page) = call(&app, "GET", &surgery, Value::Null).await;
    assert_eq!(page["items"].as_array().unwrap().len(), 1);
    assert_eq!(page["items"][0]["name"], "Tooth extraction");

    let item = format!("{collection}/{}", colour["id"].as_str().unwrap());
    let (status, patched) = call(
        &app,
        "PATCH",
        &item,
        json!({"cleanup": "PT5M", "steps": [{"duration": "PT1H", "provider1": "stylist"}]}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{patched}");
    assert_eq!(
        patched["steps"],
        json!([{"duration": "PT1H", "provider1": "stylist", "provider2": null}]),
        "a list is replaced whole"
    );
    assert_eq!(patched["cleanup"], "PT5M");
    let (status, reset) = call(&app, "PATCH", &item, json!({"cleanup": null})).await;
    assert_eq!(
        (status, reset["cleanup"].clone()),
        (StatusCode::OK, json!("P0D"))
    );
    assert_eq!(
        call(&app, "PATCH", &item, json!({"steps": null})).await.0,
        StatusCode::BAD_REQUEST,
        "steps have no default"
    );
    assert_eq!(
        call(&app, "PATCH", &item, json!({"steps": []})).await.0,
        StatusCode::BAD_REQUEST,
        "a patch is checked like a new service"
    );

    let (status, got) = call(&app, "GET", &item, Value::Null).await;
    assert_eq!(
        (status, got["name"].clone()),
        (StatusCode::OK, json!("Hair colour"))
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
