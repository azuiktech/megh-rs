//! Specification: docs/TPD-booking.md, section 13 and R6: payment methods (passes, vouchers, coupons, insurance, cash,
//! card) over REST, under `/api/v1/orgs/{org_id}/paymentMethods`.

mod booking_support;

use axum::http::StatusCode;
use booking_support::rest::*;
use booking_support::*;
use megh::booking::{booking_router, Grouped};
use serde_json::{json, Value};
use sqlx::PgPool;
use uuid::Uuid;

fn pass(uses: u32) -> Value {
    json!({"display_name": "30-visit pass", "eligible": "kind = \"massage\"", "settlement": {"prepaid": {"balance": {"uses": uses}, "valid": {"start_time": "2026-11-01T00:00:00", "end_time": "2027-02-01T00:00:00"}}}})
}

#[sqlx::test]
async fn a_prepaid_pass_is_created_with_its_balance_and_validity_in_local_time(pool: PgPool) {
    let org = clinic(&pool).await;
    let app = booking_router(pool);
    let method = created(&app, org, "paymentMethods", pass(30)).await;
    assert_eq!((method["display_name"].clone(), method["eligible"].clone(), method["org_id"].clone()), (json!("30-visit pass"), json!("kind = \"massage\""), json!(org)));
    assert_eq!(method["settlement"], json!({"prepaid": {"balance": {"uses": 30}, "valid": {"start_time": "2026-11-01T00:00:00-04:00", "end_time": "2027-02-01T00:00:00-05:00"}}}));
    assert_eq!(method["name"], json!(format!("orgs/{org}/paymentMethods/{}", method["id"].as_str().unwrap())));
}

#[sqlx::test]
async fn money_vouchers_and_cash_are_methods_too(pool: PgPool) {
    let org = clinic(&pool).await;
    let app = booking_router(pool);
    let voucher = created(&app, org, "paymentMethods", json!({"display_name": "Gift voucher", "eligible": "kind = \"massage\"", "settlement": {"prepaid": {"balance": {"money": {"currency_code": "USD", "units": "50", "nanos": 0}}}}})).await;
    assert_eq!(voucher["settlement"], json!({"prepaid": {"balance": {"money": {"currency_code": "USD", "units": "50", "nanos": 0}}, "valid": null}}));
    let cash = created(&app, org, "paymentMethods", json!({"display_name": "Cash", "eligible": "kind = \"massage\"", "settlement": "on_spot"})).await;
    assert_eq!(cash["settlement"], json!("on_spot"));
    assert_eq!(ids(&app, org, "paymentMethods", "").await, vec![voucher["id"].clone(), cash["id"].clone()]);
    assert_eq!(ids(&app, org, "paymentMethods", &filter("display_name = \"Cash\"")).await, vec![cash["id"].clone()]);
}

#[sqlx::test]
async fn a_method_can_belong_to_a_client(pool: PgPool) {
    let org = clinic(&pool).await;
    let app = booking_router(pool);
    let client = created(&app, org, "clients", json!({"display_name": "Sam"})).await;
    let mut body = pass(5);
    body["owner_id"] = client["id"].clone();
    let owned = created(&app, org, "paymentMethods", body).await;
    assert_eq!(owned["owner_id"], client["id"]);
    assert_eq!(ids(&app, org, "paymentMethods", &filter(&format!("owner_id = \"{}\"", client["id"].as_str().unwrap()))).await, vec![owned["id"].clone()]);
}

#[sqlx::test]
async fn an_update_tops_up_the_balance_and_changes_only_what_is_masked(pool: PgPool) {
    let org = clinic(&pool).await;
    let app = booking_router(pool);
    let method = created(&app, org, "paymentMethods", pass(2)).await;
    let (_, topped) = call(&app, "PATCH", &format!("{}?update_mask=settlement", path(org, "paymentMethods", &method)), Some(json!({"settlement": {"prepaid": {"balance": {"uses": 12}}}, "display_name": "ignored"}))).await;
    assert_eq!((topped["settlement"].clone(), topped["display_name"].clone()), (json!({"prepaid": {"balance": {"uses": 12}, "valid": null}}), method["display_name"].clone()));
    let (_, narrowed) = call(&app, "PATCH", &path(org, "paymentMethods", &method), Some(json!({"eligible": "kind = \"facial\""}))).await;
    assert_eq!((narrowed["eligible"].clone(), narrowed["settlement"].clone()), (json!("kind = \"facial\""), topped["settlement"].clone()));
}

#[sqlx::test]
async fn a_delete_keeps_the_method_with_its_delete_time(pool: PgPool) {
    let org = clinic(&pool).await;
    let app = booking_router(pool);
    let method = created(&app, org, "paymentMethods", pass(2)).await;
    let (status, deleted) = call(&app, "DELETE", &path(org, "paymentMethods", &method), None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(deleted["delete_time"].is_string());
    assert!(ids(&app, org, "paymentMethods", "").await.is_empty());
    assert_eq!(ids(&app, org, "paymentMethods", "?show_deleted=true").await.len(), 1);
}

#[sqlx::test]
async fn bad_input_is_an_invalid_argument_and_a_client_of_another_org_is_not_found(pool: PgPool) {
    let org = clinic(&pool).await;
    let other = clinic(&pool).await;
    let app = booking_router(pool);
    let strangers = created(&app, other, "clients", json!({"display_name": "Stranger"})).await;
    let cases = [
        json!({"eligible": "kind = \"x\"", "settlement": "on_spot"}),
        json!({"display_name": "M", "settlement": "on_spot"}),
        json!({"display_name": "M", "eligible": "kind = \"x\""}),
        json!({"display_name": "M", "eligible": "kind=\"x\"", "settlement": "on_spot"}),
        json!({"display_name": "M", "eligible": "kind = \"x\"", "settlement": "cheque"}),
        json!({"display_name": "M", "eligible": "kind = \"x\"", "settlement": {"prepaid": {}}}),
        json!({"display_name": "M", "eligible": "kind = \"x\"", "settlement": {"prepaid": {"balance": {"money": {"currency_code": "USD", "units": "-1", "nanos": 0}}}}}),
        json!({"display_name": "M", "eligible": "kind = \"x\"", "settlement": {"prepaid": {"balance": {"uses": 1}, "valid": {"start_time": "2027-01-01T00:00:00", "end_time": "2026-01-01T00:00:00"}}}}),
        json!({"display_name": "M", "eligible": "kind = \"x\"", "settlement": "on_spot", "unknown": 1}),
    ];
    for body in cases {
        let (status, error) = call(&app, "POST", &format!("/api/v1/orgs/{org}/paymentMethods"), Some(body.clone())).await;
        assert_eq!((status, error["error"]["status"].clone()), (StatusCode::BAD_REQUEST, json!("INVALID_ARGUMENT")), "{body}");
    }
    let mut owned = pass(1);
    owned["owner_id"] = strangers["id"].clone();
    assert_eq!(call(&app, "POST", &format!("/api/v1/orgs/{org}/paymentMethods"), Some(owned)).await.0, StatusCode::NOT_FOUND);
    assert_eq!(call(&app, "GET", &format!("/api/v1/orgs/{org}/paymentMethods/{}", Uuid::new_v4()), None).await.0, StatusCode::NOT_FOUND);
}

#[sqlx::test]
async fn an_org_sees_its_own_methods_and_those_of_the_orgs_grouped_under_it(pool: PgPool) {
    let (brand, location, other) = (clinic(&pool).await, clinic(&pool).await, clinic(&pool).await);
    group(&pool, Grouped::Org, location, brand).await;
    let app = booking_router(pool);
    let (own, below, elsewhere) = (created(&app, brand, "paymentMethods", pass(1)).await, created(&app, location, "paymentMethods", pass(2)).await, created(&app, other, "paymentMethods", pass(3)).await);
    assert_eq!(ids(&app, brand, "paymentMethods", "").await, vec![own["id"].clone(), below["id"].clone()]);
    assert_eq!(ids(&app, location, "paymentMethods", "").await, vec![below["id"].clone()]);
    assert_eq!(call(&app, "GET", &path(brand, "paymentMethods", &elsewhere), None).await.0, StatusCode::NOT_FOUND);
}

#[sqlx::test]
async fn a_pass_made_over_rest_pays_for_an_appointment_made_over_rest(pool: PgPool) {
    let org = clinic(&pool).await;
    let app = booking_router(pool);
    let room = created(&app, org, "bookables", json!({"display_name": "Suite", "attributes": {"kind": "room"}})).await;
    let therapist = created(&app, org, "bookables", json!({"display_name": "Tess", "attributes": {"role": "therapist"}})).await;
    for bookable in [&room, &therapist] {
        created(&app, org, "schedules", json!({"bookable_id": bookable["id"], "date": "2026-11-04", "start_time": "09:00", "end_time": "17:00"})).await;
    }
    let massage = created(&app, org, "services", json!({"display_name": "Massage", "price": {"currency_code": "USD", "units": "90", "nanos": 0}, "steps": [{"duration": "3600s", "needs": [{"who": "role = \"therapist\""}, {"who": "kind = \"room\""}]}]})).await;
    let client = created(&app, org, "clients", json!({"display_name": "Sam"})).await;
    let method = created(&app, org, "paymentMethods", json!({"display_name": "10 massages", "owner_id": client["id"], "eligible": "kind = \"massage\"", "settlement": {"prepaid": {"balance": {"uses": 10}}}})).await;
    let search = json!({"parties": [{"client_id": client["id"], "services": [{"service_id": massage["id"]}]}], "start_time": "2026-11-04T09:00:00", "end_time": "2026-11-04T10:00:00"});
    let (_, found) = call(&app, "POST", &format!("/api/v1/orgs/{org}/availability:search"), Some(search)).await;
    let booking = json!({"state": "booked", "slot": found["slots"][0], "request_id": "massage-1", "payer_id": client["id"], "payments": [{"method": method["id"], "amount": {"uses": 1}}]});
    let (status, appointment) = call(&app, "POST", &format!("/api/v1/orgs/{org}/appointments"), Some(booking)).await;
    assert_eq!(status, StatusCode::OK, "{appointment}");
    let (_, after) = call(&app, "GET", &path(org, "paymentMethods", &method), None).await;
    assert_eq!(after["settlement"], json!({"prepaid": {"balance": {"uses": 9}, "valid": null}}));
}
