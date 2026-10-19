//! Specification: clients (people, pets and others who are served) over REST under `/api/v1/orgs/{org_id}/clients`:
//! the request body is the entity, `PUT` replaces its data, `PATCH` merges the fields it names, answers are the entity
//! and a list is a page with its `data` and `next_page_token`; errors are those of Google AIP-193.

mod booking_support;

use axum::http::StatusCode;
use booking_support::rest::*;
use booking_support::*;
use megh::booking::{booking_router, Grouped};
use serde_json::json;
use sqlx::PgPool;
use uuid::Uuid;

#[sqlx::test]
async fn a_client_is_created_from_the_entity_in_the_org_of_the_path(pool: PgPool) {
    let org = clinic(&pool).await;
    let app = booking_router(pool);
    let pet = rest_client(&app, org, "Rex", json!({"kind": "dog", "breed": "beagle", "guardian": "Sam"})).await;
    assert_eq!((pet["name"].clone(), pet["org_id"].clone()), (json!("Rex"), json!(org)));
    assert_eq!(pet["attributes"], json!({"kind": "dog", "breed": "beagle", "guardian": "Sam"}));
    assert!(pet["id"].is_string() && pet["created_at"].is_string() && pet["deleted_at"].is_null());
}

#[sqlx::test]
async fn a_client_is_read_back_listed_and_filtered(pool: PgPool) {
    let org = clinic(&pool).await;
    let app = booking_router(pool);
    let rex = rest_client(&app, org, "Rex", json!({"kind": "dog"})).await;
    let tom = rest_client(&app, org, "Tom", json!({"kind": "cat"})).await;
    let (status, read) = call(&app, "GET", &path(org, "clients", &rex), None).await;
    assert_eq!((status, &read), (StatusCode::OK, &rex));
    assert_eq!(ids(&app, org, "clients", "").await, vec![rex["id"].clone(), tom["id"].clone()]);
    assert_eq!(ids(&app, org, "clients", &filter("attributes.kind = \"cat\"")).await, vec![tom["id"].clone()]);
    assert_eq!(ids(&app, org, "clients", "?page_size=1").await, vec![rex["id"].clone()]);
}

#[sqlx::test]
async fn a_put_replaces_the_data_and_a_patch_changes_only_the_fields_it_names(pool: PgPool) {
    let org = clinic(&pool).await;
    let app = booking_router(pool);
    let client = rest_client(&app, org, "Rex", json!({"kind": "dog"})).await;
    let (status, replaced) = call(&app, "PUT", &path(org, "clients", &client), Some(json!({"org_id": org, "name": "Max", "first_name": "Max", "last_name": "Client", "attributes": {"kind": "wolf"}}))).await;
    assert_eq!((status, replaced["name"].clone(), replaced["attributes"].clone()), (StatusCode::OK, json!("Max"), json!({"kind": "wolf"})));
    assert_eq!((replaced["id"].clone(), replaced["created_at"].clone()), (client["id"].clone(), client["created_at"].clone()));
    let (_, patched) = call(&app, "PATCH", &path(org, "clients", &client), Some(json!({"name": "Rex"}))).await;
    assert_eq!((patched["name"].clone(), patched["attributes"].clone()), (json!("Rex"), json!({"kind": "wolf"})));
    assert_eq!(call(&app, "PATCH", &path(org, "clients", &client), Some(json!({"name": " "}))).await.0, StatusCode::BAD_REQUEST);
}

#[sqlx::test]
async fn a_delete_keeps_the_client_with_its_deleted_at(pool: PgPool) {
    let org = clinic(&pool).await;
    let app = booking_router(pool);
    let client = rest_client(&app, org, "Rex", json!({})).await;
    let (status, deleted) = call(&app, "DELETE", &path(org, "clients", &client), None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(deleted["deleted_at"].is_string());
    assert!(ids(&app, org, "clients", "").await.is_empty());
    assert_eq!(ids(&app, org, "clients", "?show_deleted=true").await.len(), 1);
    assert_eq!(call(&app, "DELETE", &path(org, "clients", &client), None).await.0, StatusCode::NOT_FOUND);
}

#[sqlx::test]
async fn bad_input_is_an_invalid_argument_and_a_missing_client_is_not_found(pool: PgPool) {
    let org = clinic(&pool).await;
    let app = booking_router(pool);
    for body in [json!({}), json!({"name": " ", "first_name": "A", "last_name": "B", "attributes": {}}), json!({"name": 1, "attributes": {}})] {
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
    let (own, below, elsewhere) = (rest_client(&app, brand, "a", json!({})).await, rest_client(&app, location, "b", json!({})).await, rest_client(&app, other, "c", json!({})).await);
    assert_eq!(ids(&app, brand, "clients", "").await, vec![own["id"].clone(), below["id"].clone()]);
    assert_eq!(ids(&app, location, "clients", "").await, vec![below["id"].clone()]);
    assert_eq!(call(&app, "GET", &path(brand, "clients", &elsewhere), None).await.0, StatusCode::NOT_FOUND);
}

#[sqlx::test]
async fn a_client_made_over_rest_can_be_searched_for_and_booked(pool: PgPool) {
    let org = clinic(&pool).await;
    let (room, stylist) = (bookable(&pool, org, "room", 1, &[("kind", "room")]).await, bookable(&pool, org, "stylist", 1, &[("role", "stylist")]).await);
    let haircut = service(&pool, org, "haircut", 60, Some(is("role", "stylist")), is("kind", "room")).await;
    sqlx::query("UPDATE services SET org_id = $1 WHERE id = $2").bind(org).bind(haircut.id).execute(&pool).await.unwrap();
    let app = booking_router(pool);
    for bookable in [&room, &stylist] {
        let body = json!({"bookable_id": bookable.id, "date": "2026-11-04", "start_time": "09:00", "end_time": "17:00"});
        created(&app, org, "schedules", body).await;
    }
    let client = rest_client(&app, org, "Rex", json!({})).await;
    let search = json!({"parties": [{"client_id": client["id"], "services": [{"service_id": haircut.id}]}], "start_time": "2026-11-04T09:00:00", "end_time": "2026-11-04T10:00:00"});
    let (_, found) = call(&app, "POST", &format!("/api/v1/orgs/{org}/availability:search"), Some(search)).await;
    let slot = found["slots"][0].clone();
    let booking = json!({"state": "booked", "slot": slot, "request_id": "rex-1", "payer_id": client["id"]});
    let (status, appointment) = call(&app, "POST", &format!("/api/v1/orgs/{org}/appointments"), Some(booking)).await;
    assert_eq!((status, appointment["lines"][0]["client_id"].clone()), (StatusCode::OK, client["id"].clone()));
}
