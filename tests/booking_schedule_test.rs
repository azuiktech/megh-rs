//! B7 (docs/TPD-booking.md): Schedule and Blockout CRUD over HTTP: an rrule recurrence (DTSTART rule start,
//! UNTIL or COUNT rule end) and the window of each day it gives, for an optional provider and resource and any
//! listed services.

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
        Grant::new("services:*"),
        Grant::new("providers:*"),
        Grant::new("schedules:*"),
        Grant::new("blockouts:*"),
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

const MON_WED: &str =
    "DTSTART;TZID=Asia/Kolkata:20261023T000000\nRRULE:FREQ=WEEKLY;BYDAY=MO,WE;UNTIL=20261230T235959Z";

fn days(recurrence: &Value) -> Vec<String> {
    let set: rrule::RRuleSet = recurrence.as_str().unwrap().parse().unwrap();
    set.all(100).dates.iter().map(|d| d.date_naive().to_string()).collect()
}

#[sqlx::test(migrations = false)]
async fn schedule_is_a_recurrence_and_a_daily_window(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let mine = tenant(&pool).await;
    let app = app(pool, mine);
    let base = format!("/api/v1/tenants/{mine}");
    let (_, albert) = call(&app, "POST", &format!("{base}/providers"), json!({
        "first_name": "Albert", "last_name": "Hall", "name": "Dr Albert"
    }))
    .await;
    let albert = albert["id"].as_str().unwrap();
    let cleaning = Uuid::now_v7();
    let collection = format!("{base}/schedules");

    let (status, hours) = call(&app, "POST", &collection, json!({
        "name": "Clinic hours", "recurrence": MON_WED, "starts_at": "09:00:00", "ends_at": "16:00:00"
    }))
    .await;
    assert_eq!(status, StatusCode::OK, "{hours}");
    assert_eq!(
        (hours["provider_id"].clone(), hours["resource_id"].clone(), hours["service_ids"].clone()),
        (Value::Null, Value::Null, json!([])),
        "any provider, any resource, any service"
    );
    let windows = days(&hours["recurrence"]);
    assert_eq!(windows.len(), 20);
    assert_eq!(
        (windows[0].as_str(), windows[1].as_str(), windows[19].as_str()),
        ("2026-10-26", "2026-10-28", "2026-12-30"),
        "Friday 23 Oct starts the rule but is not a Monday or Wednesday"
    );
    assert_eq!((hours["starts_at"].clone(), hours["ends_at"].clone()), (json!("09:00:00"), json!("16:00:00")));

    let (status, albert_cleaning) = call(&app, "POST", &collection, json!({
        "name": "Dr Albert cleaning", "provider_id": albert, "service_ids": [cleaning],
        "recurrence": "DTSTART;TZID=Asia/Kolkata:20261021T000000\nRRULE:FREQ=WEEKLY;BYDAY=WE,TH;COUNT=4",
        "starts_at": "15:00:00", "ends_at": "18:00:00"
    }))
    .await;
    assert_eq!(status, StatusCode::OK, "{albert_cleaning}");
    assert_eq!(days(&albert_cleaning["recurrence"]), ["2026-10-21", "2026-10-22", "2026-10-28", "2026-10-29"]);

    for bad in [
        json!({"name": "X", "recurrence": "every monday", "starts_at": "09:00:00", "ends_at": "16:00:00"}),
        json!({"name": "X", "recurrence": MON_WED, "starts_at": "9 am", "ends_at": "16:00:00"}),
        json!({"name": "X", "recurrence": MON_WED, "starts_at": "22:00:00", "ends_at": "06:00:00"}),
        json!({"name": "X", "recurrence": MON_WED, "starts_at": "09:00:00", "ends_at": "09:00:00"}),
        json!({"name": "", "recurrence": MON_WED, "starts_at": "09:00:00", "ends_at": "16:00:00"}),
        json!({"name": "X", "recurrence": MON_WED, "starts_at": "09:00:00"}),
        json!({"name": "X", "provider_id": Uuid::now_v7(), "recurrence": MON_WED, "starts_at": "09:00:00", "ends_at": "16:00:00"}),
    ] {
        assert_eq!(call(&app, "POST", &collection, bad.clone()).await.0, StatusCode::BAD_REQUEST, "{bad}");
    }

    let by_provider = format!("{collection}?filter=provider_id%20%3D%20%22{albert}%22");
    let (_, page) = call(&app, "GET", &by_provider, Value::Null).await;
    assert_eq!(page["items"].as_array().unwrap().len(), 1);
    let by_service = format!("{collection}?filter=service_ids%3A%22{cleaning}%22");
    let (_, page) = call(&app, "GET", &by_service, Value::Null).await;
    assert_eq!(page["items"][0]["name"], "Dr Albert cleaning");

    let item = format!("{collection}/{}", hours["id"].as_str().unwrap());
    let (status, patched) = call(&app, "PATCH", &item, json!({"ends_at": "17:00:00", "provider_id": albert})).await;
    assert_eq!((status, patched["ends_at"].clone(), patched["provider_id"].clone()), (StatusCode::OK, json!("17:00:00"), json!(albert)));
    assert_eq!(
        call(&app, "PATCH", &item, json!({"ends_at": "08:00:00"})).await.0,
        StatusCode::BAD_REQUEST,
        "a patch is checked like a new schedule"
    );
    assert_eq!(call(&app, "DELETE", &item, Value::Null).await.0, StatusCode::OK);
    assert_eq!(call(&app, "GET", &item, Value::Null).await.0, StatusCode::NOT_FOUND);
}

#[sqlx::test(migrations = false)]
async fn blockout_is_a_schedule_with_a_reason(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let mine = tenant(&pool).await;
    let app = app(pool, mine);
    let collection = format!("/api/v1/tenants/{mine}/blockouts");

    let (status, lunch) = call(&app, "POST", &collection, json!({
        "name": "Lunch", "recurrence": MON_WED, "starts_at": "13:00:00", "ends_at": "14:00:00", "reason": "lunch break"
    }))
    .await;
    assert_eq!(status, StatusCode::OK, "{lunch}");
    assert_eq!(lunch["reason"], "lunch break");
    assert_eq!(days(&lunch["recurrence"]).len(), 20);

    let (status, holiday) = call(&app, "POST", &collection, json!({
        "name": "Christmas", "starts_at": "00:00:00", "ends_at": "23:59:59",
        "recurrence": "DTSTART;TZID=Asia/Kolkata:20261225T000000\nRRULE:FREQ=YEARLY;COUNT=1"
    }))
    .await;
    assert_eq!(status, StatusCode::OK, "{holiday}");
    assert_eq!((holiday["reason"].clone(), days(&holiday["recurrence"])), (json!(""), vec!["2026-12-25".to_string()]));

    assert_eq!(
        call(&app, "POST", &collection, json!({
            "name": "X", "recurrence": MON_WED, "starts_at": "14:00:00", "ends_at": "13:00:00"
        })).await.0,
        StatusCode::BAD_REQUEST
    );
    let (_, page) = call(&app, "GET", &collection, Value::Null).await;
    assert_eq!(page["items"].as_array().unwrap().len(), 2);
}
