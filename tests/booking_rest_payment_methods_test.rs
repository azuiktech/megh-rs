//! Specification: payment methods (passes, vouchers, coupons, insurance, cash, card) over REST under
//! `/api/v1/orgs/{org_id}/paymentMethods`: the request body is the entity, `PUT` replaces its data, `PATCH` merges the
//! fields it names, answers are the entity and a list is a page with its `data` and `next_page_token`.

mod booking_support;

use axum::http::StatusCode;
use booking_support::rest::*;
use booking_support::*;
use megh::booking::{booking_router, Grouped};
use serde_json::{json, Value};
use sqlx::PgPool;
use uuid::Uuid;

fn eligible(filter: &str) -> Value {
    json!({"Filter": filter})
}

fn pass(org: Uuid, uses: u32) -> Value {
    let valid = json!({"start": "2026-11-01T04:00:00Z", "end": "2027-02-01T05:00:00Z"});
    method(org, "30-visit pass", json!({"type": "prepaid", "value": {"balance": {"type": "uses", "value": uses}, "valid": valid}}))
}

fn method(org: Uuid, name: &str, settlement: Value) -> Value {
    json!({"org_id": org, "name": name, "owner_id": null, "eligible": eligible("kind = \"massage\""), "settlement": settlement, "attributes": {}})
}

#[sqlx::test]
async fn a_prepaid_pass_is_created_from_the_entity_with_its_balance_and_validity(pool: PgPool) {
    let org = clinic(&pool).await;
    let app = booking_router(pool);
    let created = created(&app, org, "paymentMethods", pass(org, 30)).await;
    assert_eq!((created["name"].clone(), created["eligible"].clone(), created["org_id"].clone()), (json!("30-visit pass"), eligible("kind = \"massage\""), json!(org)));
    assert_eq!(created["settlement"], pass(org, 30)["settlement"]);
    assert!(created["id"].is_string() && created["deleted_at"].is_null());
}

#[sqlx::test]
async fn money_vouchers_and_cash_are_methods_too(pool: PgPool) {
    let org = clinic(&pool).await;
    let app = booking_router(pool);
    let money = json!({"type": "money", "value": {"micros": 50_000_000, "currency": "USD"}});
    let voucher = created(&app, org, "paymentMethods", method(org, "Gift voucher", json!({"type": "prepaid", "value": {"balance": money, "valid": null}}))).await;
    let cash = created(&app, org, "paymentMethods", method(org, "Cash", json!({"type": "on_spot"}))).await;
    assert_eq!(cash["settlement"], json!({"type": "on_spot"}));
    assert_eq!(ids(&app, org, "paymentMethods", "").await, vec![voucher["id"].clone(), cash["id"].clone()]);
    assert_eq!(ids(&app, org, "paymentMethods", &filter("name = \"Cash\"")).await, vec![cash["id"].clone()]);
}

#[sqlx::test]
async fn a_method_can_belong_to_a_client(pool: PgPool) {
    let org = clinic(&pool).await;
    let app = booking_router(pool);
    let client = rest_client(&app, org, "Sam", json!({})).await;
    let mut body = pass(org, 5);
    body["owner_id"] = client["id"].clone();
    let owned = created(&app, org, "paymentMethods", body).await;
    assert_eq!(owned["owner_id"], client["id"]);
    assert_eq!(ids(&app, org, "paymentMethods", &filter(&format!("owner_id = \"{}\"", client["id"].as_str().unwrap()))).await, vec![owned["id"].clone()]);
}

#[sqlx::test]
async fn a_put_tops_up_the_balance_and_a_patch_changes_only_the_fields_it_names(pool: PgPool) {
    let org = clinic(&pool).await;
    let app = booking_router(pool);
    let created = created(&app, org, "paymentMethods", pass(org, 2)).await;
    let (status, topped) = call(&app, "PUT", &path(org, "paymentMethods", &created), Some(pass(org, 12))).await;
    assert_eq!((status, topped["settlement"].clone(), topped["id"].clone()), (StatusCode::OK, pass(org, 12)["settlement"].clone(), created["id"].clone()));
    let (_, narrowed) = call(&app, "PATCH", &path(org, "paymentMethods", &created), Some(json!({"eligible": eligible("kind = \"facial\"")}))).await;
    assert_eq!((narrowed["eligible"].clone(), narrowed["settlement"].clone()), (eligible("kind = \"facial\""), topped["settlement"].clone()));
}

#[sqlx::test]
async fn a_delete_keeps_the_method_with_its_deleted_at(pool: PgPool) {
    let org = clinic(&pool).await;
    let app = booking_router(pool);
    let created = created(&app, org, "paymentMethods", pass(org, 2)).await;
    let (status, deleted) = call(&app, "DELETE", &path(org, "paymentMethods", &created), None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(deleted["deleted_at"].is_string());
    assert!(ids(&app, org, "paymentMethods", "").await.is_empty());
    assert_eq!(ids(&app, org, "paymentMethods", "?show_deleted=true").await.len(), 1);
}

#[sqlx::test]
async fn bad_input_is_an_invalid_argument_and_a_client_of_another_org_is_not_found(pool: PgPool) {
    let org = clinic(&pool).await;
    let other = clinic(&pool).await;
    let app = booking_router(pool);
    let strangers = rest_client(&app, other, "Stranger", json!({})).await;
    let with = |field: &str, value: Value| {
        let mut body = method(org, "M", json!({"type": "on_spot"}));
        body[field] = value;
        body
    };
    let prepaid = |value: Value| json!({"type": "prepaid", "value": value});
    let cases = [
        json!({"org_id": org, "eligible": eligible("kind = \"x\""), "settlement": {"type": "on_spot"}, "attributes": {}}),
        with("name", json!(" ")),
        with("org_id", json!(other)),
        with("eligible", eligible("kind=\"x\"")),
        with("settlement", json!({"type": "cheque"})),
        with("settlement", prepaid(json!({}))),
        with("settlement", prepaid(json!({"balance": {"type": "uses", "value": 1}, "valid": {"start": "2027-01-01T00:00:00Z", "end": "2026-01-01T00:00:00Z"}}))),
    ];
    for body in cases {
        let (status, error) = call(&app, "POST", &format!("/api/v1/orgs/{org}/paymentMethods"), Some(body.clone())).await;
        assert_eq!((status, error["error"]["status"].clone()), (StatusCode::BAD_REQUEST, json!("INVALID_ARGUMENT")), "{body}");
    }
    let mut owned = pass(org, 1);
    owned["owner_id"] = strangers["id"].clone();
    assert_eq!(call(&app, "POST", &format!("/api/v1/orgs/{org}/paymentMethods"), Some(owned)).await.0, StatusCode::NOT_FOUND);
    assert_eq!(call(&app, "GET", &format!("/api/v1/orgs/{org}/paymentMethods/{}", Uuid::new_v4()), None).await.0, StatusCode::NOT_FOUND);
}

#[sqlx::test]
async fn an_org_sees_its_own_methods_and_those_of_the_orgs_grouped_under_it(pool: PgPool) {
    let (brand, location, other) = (clinic(&pool).await, clinic(&pool).await, clinic(&pool).await);
    group(&pool, Grouped::Org, location, brand).await;
    let app = booking_router(pool);
    let (own, below, elsewhere) = (created(&app, brand, "paymentMethods", pass(brand, 1)).await, created(&app, location, "paymentMethods", pass(location, 2)).await, created(&app, other, "paymentMethods", pass(other, 3)).await);
    assert_eq!(ids(&app, brand, "paymentMethods", "").await, vec![own["id"].clone(), below["id"].clone()]);
    assert_eq!(ids(&app, location, "paymentMethods", "").await, vec![below["id"].clone()]);
    assert_eq!(call(&app, "GET", &path(brand, "paymentMethods", &elsewhere), None).await.0, StatusCode::NOT_FOUND);
}
