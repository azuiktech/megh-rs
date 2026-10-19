//! Specification: docs/TPD-booking.md, D9 and section 10: groupings (a brand and its locations, a lobby and its
//! rooms, a category and its services) over REST, under `/api/v1/orgs/{org_id}/groupings`.

mod booking_support;

use axum::http::StatusCode;
use booking_support::rest::*;
use booking_support::*;
use chrono::{TimeZone, Utc};
use megh::booking::{booking_router, Availability, Axes, Calendar, Interval};
use serde_json::{json, Value};
use sqlx::PgPool;
use uuid::Uuid;

fn edge(kind: &str, member: impl serde::Serialize, group: impl serde::Serialize) -> Value {
    json!({"kind": kind, "member_id": member, "group_id": group})
}

#[sqlx::test]
async fn an_edge_puts_a_member_under_a_group(pool: PgPool) {
    let (brand, location) = (clinic(&pool).await, clinic(&pool).await);
    let app = booking_router(pool);
    let grouping = created(&app, location, "groupings", edge("org", location, brand)).await;
    assert_eq!((grouping["kind"].clone(), grouping["member_id"].clone(), grouping["group_id"].clone(), grouping["org_id"].clone()), (json!("org"), json!(location), json!(brand), json!(location)));
    assert_eq!(grouping["name"], json!(format!("orgs/{location}/groupings/{}", grouping["id"].as_str().unwrap())));
    assert_eq!(call(&app, "GET", &path(location, "groupings", &grouping), None).await, (StatusCode::OK, grouping.clone()));
    assert_eq!(ids(&app, location, "groupings", "").await, vec![grouping["id"].clone()]);
    assert_eq!(ids(&app, brand, "groupings", "").await, vec![grouping["id"].clone()]);
}

#[sqlx::test]
async fn bookables_services_and_clients_are_grouped_too(pool: PgPool) {
    let org = clinic(&pool).await;
    let app = booking_router(pool);
    let lobby = created(&app, org, "bookables", json!({"display_name": "Lobby"})).await;
    let room = created(&app, org, "bookables", json!({"display_name": "Room 1"})).await;
    let hair = created(&app, org, "services", json!({"display_name": "Hair", "price": {"currency_code": "USD", "units": "0", "nanos": 0}})).await;
    let colour = created(&app, org, "services", json!({"display_name": "Colour", "price": {"currency_code": "USD", "units": "9", "nanos": 0}})).await;
    let vips = created(&app, org, "clients", json!({"display_name": "VIPs", "first_name": "VIPs", "last_name": "Client"})).await;
    let sam = created(&app, org, "clients", json!({"display_name": "Sam", "first_name": "Sam", "last_name": "Client"})).await;
    let rooms = created(&app, org, "groupings", edge("bookable", &room["id"], &lobby["id"])).await;
    let menu = created(&app, org, "groupings", edge("service", &colour["id"], &hair["id"])).await;
    let people = created(&app, org, "groupings", edge("client", &sam["id"], &vips["id"])).await;
    assert_eq!(ids(&app, org, "groupings", &filter("kind = \"service\"")).await, vec![menu["id"].clone()]);
    assert_eq!(ids(&app, org, "groupings", "").await, vec![rooms["id"].clone(), menu["id"].clone(), people["id"].clone()]);
}

#[sqlx::test]
async fn an_edge_made_over_rest_changes_what_the_calendar_gives_and_a_delete_undoes_it(pool: PgPool) {
    let (brand, location) = (clinic(&pool).await, clinic(&pool).await);
    let room = bookable(&pool, location, "room", 1, &[]).await;
    let cleaning = service(&pool, brand, "cleaning", 30, None, is("kind", "room")).await;
    let app = booking_router(pool.clone());
    let hours = json!({"org_id": brand, "date": "2026-11-12", "start_time": "09:00", "end_time": "10:00"});
    created(&app, brand, "schedules", hours).await;
    let axes = Axes { org: location, service: cleaning.id, bookable: room.id };
    let day = Interval { start: Utc.with_ymd_and_hms(2026, 11, 12, 0, 0, 0).unwrap(), end: Utc.with_ymd_and_hms(2026, 11, 13, 0, 0, 0).unwrap() };
    let calendar = Calendar::new(pool);
    assert!(calendar.open(&axes, &day).await.unwrap().is_empty());
    let grouping = created(&app, location, "groupings", edge("org", location, brand)).await;
    assert_eq!(calendar.open(&axes, &day).await.unwrap().len(), 1);
    assert_eq!(call(&app, "DELETE", &path(location, "groupings", &grouping), None).await.0, StatusCode::OK);
    assert!(calendar.open(&axes, &day).await.unwrap().is_empty());
}

#[sqlx::test]
async fn what_would_break_the_hierarchy_is_refused(pool: PgPool) {
    let (brand, franchise, location, other) = (clinic(&pool).await, clinic(&pool).await, clinic(&pool).await, clinic(&pool).await);
    let app = booking_router(pool);
    created(&app, franchise, "groupings", edge("org", franchise, brand)).await;
    created(&app, location, "groupings", edge("org", location, franchise)).await;
    let under = |org: Uuid| format!("/api/v1/orgs/{org}/groupings");
    let cases = [
        (brand, edge("org", brand, location), StatusCode::BAD_REQUEST, "FAILED_PRECONDITION"),
        (brand, edge("org", brand, brand), StatusCode::BAD_REQUEST, "INVALID_ARGUMENT"),
        (location, edge("org", location, franchise), StatusCode::CONFLICT, "ALREADY_EXISTS"),
        (brand, edge("org", other, brand), StatusCode::NOT_FOUND, "NOT_FOUND"),
        (location, edge("org", location, Uuid::new_v4()), StatusCode::NOT_FOUND, "NOT_FOUND"),
        (brand, edge("org", Uuid::new_v4(), brand), StatusCode::NOT_FOUND, "NOT_FOUND"),
        (brand, edge("bookable", location, franchise), StatusCode::NOT_FOUND, "NOT_FOUND"),
        (brand, json!({"kind": "org", "member_id": location}), StatusCode::BAD_REQUEST, "INVALID_ARGUMENT"),
        (brand, json!({"kind": "team", "member_id": location, "group_id": brand}), StatusCode::BAD_REQUEST, "INVALID_ARGUMENT"),
        (brand, json!({"kind": "org", "member_id": location, "group_id": brand, "x": 1}), StatusCode::BAD_REQUEST, "INVALID_ARGUMENT"),
    ];
    for (org, body, expected, name) in cases {
        let (status, error) = call(&app, "POST", &under(org), Some(body.clone())).await;
        assert_eq!((status, error["error"]["status"].clone()), (expected, json!(name)), "{body}");
    }
}

#[sqlx::test]
async fn an_edge_cannot_be_changed_and_a_deleted_one_can_be_made_again(pool: PgPool) {
    let (brand, location) = (clinic(&pool).await, clinic(&pool).await);
    let app = booking_router(pool);
    let grouping = created(&app, location, "groupings", edge("org", location, brand)).await;
    let (status, _) = call(&app, "PATCH", &path(location, "groupings", &grouping), Some(json!({}))).await;
    assert_eq!(status, StatusCode::METHOD_NOT_ALLOWED);
    let (_, deleted) = call(&app, "DELETE", &path(location, "groupings", &grouping), None).await;
    assert!(deleted["delete_time"].is_string());
    assert!(ids(&app, location, "groupings", "").await.is_empty());
    assert_eq!(ids(&app, location, "groupings", "?show_deleted=true").await.len(), 1);
    created(&app, location, "groupings", edge("org", location, brand)).await;
}

#[sqlx::test]
async fn an_org_sees_only_its_own_edges_and_those_of_the_orgs_below_it(pool: PgPool) {
    let (brand, franchise, location, other, stranger) = (clinic(&pool).await, clinic(&pool).await, clinic(&pool).await, clinic(&pool).await, clinic(&pool).await);
    let app = booking_router(pool);
    let top = created(&app, franchise, "groupings", edge("org", franchise, brand)).await;
    let below = created(&app, location, "groupings", edge("org", location, franchise)).await;
    let elsewhere = created(&app, stranger, "groupings", edge("org", stranger, other)).await;
    assert_eq!(ids(&app, brand, "groupings", "").await, vec![top["id"].clone(), below["id"].clone()]);
    assert_eq!(ids(&app, franchise, "groupings", "").await, vec![top["id"].clone(), below["id"].clone()]);
    assert_eq!(ids(&app, location, "groupings", "").await, vec![below["id"].clone()]);
    assert_eq!(call(&app, "GET", &path(brand, "groupings", &elsewhere), None).await.0, StatusCode::NOT_FOUND);
}
