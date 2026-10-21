//! B8 (docs/TPD-booking.md): Appointment over HTTP. No create route; status moves only through the AIP-136
//! custom methods `:book`, `:confirm`, `:complete` and DELETE (cancel); PATCH changes note, payment and
//! attributes, ignores `status` and `booked_at`.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::middleware::from_fn;
use axum::{Extension, Router};
use megh::auth::rest::AppContext;
use megh::{Grant, authorizer};
use serde_json::{Value, json};
use sqlx::PgPool;
use tower::util::MapRequestLayer;
use tower::{Layer, ServiceExt};
use uuid::Uuid;

async fn org(pool: &PgPool) -> Uuid {
    sqlx::query_scalar("INSERT INTO organizations (name) VALUES ('Clinic') RETURNING id")
        .fetch_one(pool)
        .await
        .unwrap()
}

fn app(pool: PgPool, org: Uuid, grants: &[&str]) -> Router {
    let (api, _) = megh::booking::router().split_for_parts();
    let grants = [format!("orgs:read:{org}")]
        .into_iter()
        .chain(grants.iter().map(|grant| grant.to_string()))
        .map(Grant::new)
        .collect::<Vec<_>>();
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
    let service = MapRequestLayer::new(megh::booking::rewrite_aip_136).layer(app.clone());
    let response = service.oneshot(request.unwrap()).await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

/// A client, a service and a room of the org, and an appointment in `status` stored as booking would store it.
async fn appointment(pool: &PgPool, org: Uuid, status: &str) -> Uuid {
    let client: Uuid = sqlx::query_scalar(
        "INSERT INTO clients (org_id, id, first_name, last_name, name) VALUES ($1, $2, 'Ada', 'L', 'Ada') RETURNING id",
    )
    .bind(org)
    .bind(Uuid::now_v7())
    .fetch_one(pool)
    .await
    .unwrap();
    let service: Uuid = sqlx::query_scalar(
        r#"INSERT INTO services (org_id, id, name, steps) VALUES ($1, $2, 'Cleaning', '[{"duration": "PT30M"}]') RETURNING id"#,
    )
    .bind(org)
    .bind(Uuid::now_v7())
    .fetch_one(pool)
    .await
    .unwrap();
    let room: Uuid = sqlx::query_scalar("INSERT INTO resources (org_id, id, name) VALUES ($1, $2, 'Room 1') RETURNING id")
        .bind(org)
        .bind(Uuid::now_v7())
        .fetch_one(pool)
        .await
        .unwrap();
    sqlx::query_scalar(
        "INSERT INTO appointments (org_id, id, client_id, service_id, resource_id, starts_at, ends_at, status, payment) \
         VALUES ($1, $2, $3, $4, $5, '2026-10-26T09:30:00Z', '2026-10-26T10:00:00Z', $6, $7) RETURNING id",
    )
    .bind(org)
    .bind(Uuid::now_v7())
    .bind(client)
    .bind(service)
    .bind(room)
    .bind(status)
    .bind(json!({"payer_id": client, "method": "card"}))
    .fetch_one(pool)
    .await
    .unwrap()
}

#[sqlx::test(migrations = false)]
async fn status_moves_only_along_the_allowed_transitions(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let mine = org(&pool).await;
    let waiting = appointment(&pool, mine, "waitlist").await;
    let booked = appointment(&pool, mine, "booked").await;
    sqlx::query(
        r#"INSERT INTO schedules (org_id, id, name, recurrence, starts_at, ends_at)
           VALUES ($1, $2, 'Open', '"DTSTART:20261026T000000Z\nRRULE:FREQ=DAILY;COUNT=1"', '00:00', '23:00')"#,
    )
    .bind(mine)
    .bind(Uuid::now_v7())
    .execute(&pool)
    .await
    .unwrap();
    let app = app(pool, mine, &["appointments:*"]);
    let item = |id: Uuid| format!("/api/v1/orgs/{mine}/appointments/{id}");

    let (status, got) = call(&app, "GET", &item(waiting), Value::Null).await;
    assert_eq!((status, got["status"].clone()), (StatusCode::OK, json!("waitlist")), "{got}");
    assert!(got["booked_at"].is_string(), "set by the server for a waitlisted appointment too");

    let step = |id: Uuid, verb: &str| format!("{}:{verb}", item(id));
    assert_eq!(call(&app, "POST", &step(waiting, "confirm"), json!({})).await.0, StatusCode::BAD_REQUEST);
    assert_eq!(call(&app, "POST", &step(waiting, "complete"), json!({})).await.0, StatusCode::BAD_REQUEST);

    let insurance = json!({"payer_id": Uuid::now_v7(), "method": "group insurance", "attributes": {"insurer": "Acme"}});
    let (status, now_booked) = call(&app, "POST", &step(waiting, "book"), json!({"payment": insurance})).await;
    assert_eq!((status, now_booked["status"].clone()), (StatusCode::OK, json!("booked")), "{now_booked}");
    assert_eq!(now_booked["payment"], insurance, "the payment given replaces the stored one");

    let (status, confirmed) = call(&app, "POST", &step(waiting, "confirm"), Value::Null).await;
    assert_eq!((status, confirmed["status"].clone()), (StatusCode::OK, json!("confirmed")), "{confirmed}");
    assert_eq!(confirmed["payment"], insurance, "no payment given keeps the stored one");
    assert_eq!(call(&app, "POST", &step(waiting, "book"), json!({})).await.0, StatusCode::BAD_REQUEST);

    let (status, completed) = call(&app, "POST", &step(waiting, "complete"), json!({})).await;
    assert_eq!((status, completed["status"].clone()), (StatusCode::OK, json!("completed")));
    assert_eq!(call(&app, "DELETE", &item(waiting), Value::Null).await.0, StatusCode::BAD_REQUEST, "completed cannot be cancelled");

    let (status, cancelled) = call(&app, "DELETE", &item(booked), Value::Null).await;
    assert_eq!((status, cancelled["status"].clone()), (StatusCode::OK, json!("cancelled")));
    let (status, kept) = call(&app, "GET", &item(booked), Value::Null).await;
    assert_eq!((status, kept["status"].clone()), (StatusCode::OK, json!("cancelled")), "cancel is a status, not a delete");
    assert_eq!(call(&app, "POST", &step(booked, "book"), json!({})).await.0, StatusCode::BAD_REQUEST);
    assert_eq!(call(&app, "DELETE", &item(booked), Value::Null).await.0, StatusCode::BAD_REQUEST);

    assert_eq!(call(&app, "POST", &step(Uuid::now_v7(), "confirm"), json!({})).await.0, StatusCode::NOT_FOUND);
    assert_eq!(call(&app, "POST", &step(booked, "cancel"), json!({})).await.0, StatusCode::BAD_REQUEST, "not a custom method");
    assert_eq!(
        call(&app, "POST", &format!("/api/v1/orgs/{mine}/appointments"), json!({})).await.0,
        StatusCode::BAD_REQUEST,
        "booking takes the whole appointment"
    );
}

#[sqlx::test(migrations = false)]
async fn patch_changes_note_payment_and_attributes_only(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let mine = org(&pool).await;
    let booked = appointment(&pool, mine, "booked").await;
    let app = app(pool, mine, &["appointments:*"]);
    let item = format!("/api/v1/orgs/{mine}/appointments/{booked}");
    let (_, before) = call(&app, "GET", &item, Value::Null).await;

    let (status, patched) = call(&app, "PATCH", &item, json!({
        "note": "bring x-rays", "attributes": {"first_visit": true},
        "status": "confirmed", "booked_at": "2000-01-01T00:00:00Z"
    }))
    .await;
    assert_eq!(status, StatusCode::OK, "{patched}");
    assert_eq!((patched["note"].clone(), patched["attributes"]["first_visit"].clone()), (json!("bring x-rays"), json!(true)));
    assert_eq!(
        (patched["status"].clone(), patched["booked_at"].clone()),
        (before["status"].clone(), before["booked_at"].clone()),
        "status and booked_at sent are ignored"
    );

    assert_eq!(
        call(&app, "PATCH", &item, json!({"starts_at": "2026-10-27T09:30:00Z"})).await.0,
        StatusCode::BAD_REQUEST,
        "a move needs availability search"
    );
    assert_eq!(
        call(&app, "PATCH", &item, json!({"payment": {"payer_id": Uuid::now_v7(), "method": ""}})).await.0,
        StatusCode::BAD_REQUEST
    );

    let filtered = format!("/api/v1/orgs/{mine}/appointments?filter=status%20%3D%20%22booked%22");
    let (_, page) = call(&app, "GET", &filtered, Value::Null).await;
    assert_eq!(page["items"].as_array().unwrap().len(), 1);
}

#[sqlx::test(migrations = false)]
async fn each_custom_method_needs_its_own_grant(pool: PgPool) {
    megh::migrate(&pool).await.unwrap();
    let mine = org(&pool).await;
    let booked = appointment(&pool, mine, "booked").await;
    let crud_only = app(pool.clone(), mine, &["appointments:read", "appointments:create", "appointments:update"]);
    let step = |verb: &str| format!("/api/v1/orgs/{mine}/appointments/{booked}:{verb}");

    assert_eq!(call(&crud_only, "POST", &step("confirm"), json!({})).await.0, StatusCode::FORBIDDEN);
    assert_eq!(
        call(&crud_only, "POST", &format!("/api/v1/orgs/{mine}/appointments/{booked}/:confirm"), json!({})).await.0,
        StatusCode::NOT_FOUND,
        "only the public form {{id}}:confirm reaches the route"
    );
    let confirmer = app(pool, mine, &[&format!("appointments:confirm:{booked}")]);
    assert_eq!(call(&confirmer, "POST", &step("complete"), json!({})).await.0, StatusCode::FORBIDDEN);
    assert_eq!(call(&confirmer, "POST", &step("confirm"), json!({})).await.0, StatusCode::OK);
}
