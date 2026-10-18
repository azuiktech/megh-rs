//! Specification: docs/TPD-booking.md, R6 and R13: clients (people, pets and others who are served) over REST, in the
//! manner of Google AIP-131 to 135, 148, 160, 164 and 193, under `/api/v1/orgs/{org_id}/clients`.

mod booking_support;

use axum::http::StatusCode;
use booking_support::rest::*;
use booking_support::*;
use megh::booking::{booking_router, Grouped};
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

#[sqlx::test]
async fn a_client_is_created_with_its_tags_in_the_org_of_the_path(pool: PgPool) {
    let org = clinic(&pool).await;
    let app = booking_router(pool);
    let pet = created(&app, org, "clients", json!({"display_name": "Rex", "attributes": {"kind": "dog", "breed": "beagle", "guardian": "Sam"}})).await;
    assert_eq!((pet["display_name"].clone(), pet["org_id"].clone()), (json!("Rex"), json!(org)));
    assert_eq!(pet["attributes"], json!({"kind": "dog", "breed": "beagle", "guardian": "Sam"}));
    assert_eq!(pet["name"], json!(format!("orgs/{org}/clients/{}", pet["id"].as_str().unwrap())));
    assert!(pet["delete_time"].is_null());
}

#[sqlx::test]
async fn a_client_is_read_back_listed_and_filtered(pool: PgPool) {
    let org = clinic(&pool).await;
    let app = booking_router(pool);
    let rex = created(&app, org, "clients", json!({"display_name": "Rex", "attributes": {"kind": "dog"}})).await;
    let tom = created(&app, org, "clients", json!({"display_name": "Tom", "attributes": {"kind": "cat"}})).await;
    let (status, read) = call(&app, "GET", &path(org, "clients", &rex), None).await;
    assert_eq!((status, &read), (StatusCode::OK, &rex));
    assert_eq!(ids(&app, org, "clients", "").await, vec![rex["id"].clone(), tom["id"].clone()]);
    assert_eq!(ids(&app, org, "clients", &filter("attributes.kind = \"cat\"")).await, vec![tom["id"].clone()]);
    assert_eq!(ids(&app, org, "clients", "?page_size=1").await, vec![rex["id"].clone()]);
}

#[sqlx::test]
async fn an_update_changes_the_masked_fields_only(pool: PgPool) {
    let org = clinic(&pool).await;
    let app = booking_router(pool);
    let client = created(&app, org, "clients", json!({"display_name": "Rex", "attributes": {"kind": "dog"}})).await;
    let (_, masked) = call(&app, "PATCH", &format!("{}?update_mask=attributes", path(org, "clients", &client)), Some(json!({"attributes": {"kind": "wolf"}, "display_name": "ignored"}))).await;
    assert_eq!((masked["display_name"].clone(), masked["attributes"].clone()), (json!("Rex"), json!({"kind": "wolf"})));
    let (_, unmasked) = call(&app, "PATCH", &path(org, "clients", &client), Some(json!({"display_name": "Max"}))).await;
    assert_eq!((unmasked["display_name"].clone(), unmasked["attributes"].clone()), (json!("Max"), json!({"kind": "wolf"})));
}

#[sqlx::test]
async fn a_delete_keeps_the_client_with_its_delete_time(pool: PgPool) {
    let org = clinic(&pool).await;
    let app = booking_router(pool);
    let client = created(&app, org, "clients", json!({"display_name": "Rex"})).await;
    let (status, deleted) = call(&app, "DELETE", &path(org, "clients", &client), None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(deleted["delete_time"].is_string());
    assert!(ids(&app, org, "clients", "").await.is_empty());
    assert_eq!(ids(&app, org, "clients", "?show_deleted=true").await.len(), 1);
    assert_eq!(call(&app, "DELETE", &path(org, "clients", &client), None).await.0, StatusCode::NOT_FOUND);
}

#[sqlx::test]
async fn bad_input_is_an_invalid_argument_and_a_missing_client_is_not_found(pool: PgPool) {
    let org = clinic(&pool).await;
    let other = clinic(&pool).await;
    let app = booking_router(pool);
    for body in [json!({}), json!({"display_name": ""}), json!({"display_name": "Rex", "x": 1}), json!({"display_name": "Rex", "org_id": other})] {
        let (status, error) = call(&app, "POST", &format!("/api/v1/orgs/{org}/clients"), Some(body.clone())).await;
        assert_eq!((status, error["error"]["status"].clone()), (StatusCode::BAD_REQUEST, json!("INVALID_ARGUMENT")), "{body}");
    }
    assert_eq!(call(&app, "GET", &format!("/api/v1/orgs/{org}/clients/{}", Uuid::new_v4()), None).await.0, StatusCode::NOT_FOUND);
}

#[sqlx::test]
async fn an_org_sees_its_own_clients_and_those_of_the_orgs_grouped_under_it(pool: PgPool) {
    let (brand, location, other) = (clinic(&pool).await, clinic(&pool).await, clinic(&pool).await);
    group(&pool, Grouped::Org, location, brand).await;
    let app = booking_router(pool);
    let (own, below, elsewhere) = (created(&app, brand, "clients", json!({"display_name": "a"})).await, created(&app, location, "clients", json!({"display_name": "b"})).await, created(&app, other, "clients", json!({"display_name": "c"})).await);
    assert_eq!(ids(&app, brand, "clients", "").await, vec![own["id"].clone(), below["id"].clone()]);
    assert_eq!(ids(&app, location, "clients", "").await, vec![below["id"].clone()]);
    assert_eq!(call(&app, "GET", &path(brand, "clients", &elsewhere), None).await.0, StatusCode::NOT_FOUND);
}

#[sqlx::test]
async fn a_client_made_over_rest_can_be_searched_for_and_booked(pool: PgPool) {
    let org = clinic(&pool).await;
    let (room, stylist) = (bookable(&pool, org, "room", 1, &[("kind", "room")]).await, bookable(&pool, org, "stylist", 1, &[("role", "stylist")]).await);
    let haircut = service(&pool, "haircut", 60, Some(is("role", "stylist")), is("kind", "room")).await;
    sqlx::query("UPDATE services SET org_id = $1 WHERE id = $2").bind(org).bind(haircut.id).execute(&pool).await.unwrap();
    let app = booking_router(pool);
    for bookable in [&room, &stylist] {
        let body = json!({"bookable_id": bookable.id, "date": "2026-11-04", "start_time": "09:00", "end_time": "17:00"});
        created(&app, org, "schedules", body).await;
    }
    let client = created(&app, org, "clients", json!({"display_name": "Rex"})).await;
    let search = json!({"parties": [{"client_id": client["id"], "services": [{"service_id": haircut.id}]}], "start_time": "2026-11-04T09:00:00", "end_time": "2026-11-04T10:00:00"});
    let (_, found) = call(&app, "POST", &format!("/api/v1/orgs/{org}/availability:search"), Some(search)).await;
    let slot = found["slots"][0].clone();
    let booking = json!({"state": "booked", "slot": slot, "request_id": "rex-1", "payer_id": client["id"]});
    let (status, appointment) = call(&app, "POST", &format!("/api/v1/orgs/{org}/appointments"), Some(booking)).await;
    assert_eq!((status, appointment["lines"][0]["client_id"].clone()), (StatusCode::OK, client["id"].clone()));
}
