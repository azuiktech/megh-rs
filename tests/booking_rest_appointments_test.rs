//! Specification: docs/TPD-booking.md, R6: appointments over REST, held and booked as one resource with a `state`,
//! confirmed with a custom method (AIP-136), retry-safe with a `request_id` (AIP-155), cancelled by a soft delete (AIP-164).

mod booking_support;

use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use axum::Router;
use booking_support::*;
use megh::booking::{booking_router, Amount, BookableRow, ClientRow, Grouped, PaymentMethod, PaymentMethodId, PaymentMethodRow, ServiceRow, Settlement};
use serde_json::{json, Value};
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

async fn call(app: &Router, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
    let request = Request::builder().method(method).uri(uri).header("content-type", "application/json").header("sec-fetch-site", "same-origin");
    let request = request.body(body.map_or_else(Body::empty, |body| Body::from(body.to_string()))).unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    (status, serde_json::from_slice(&to_bytes(response.into_body(), usize::MAX).await.unwrap()).unwrap_or(Value::Null))
}

struct Salon {
    app: Router,
    org: Uuid,
    stylist: BookableRow,
    haircut: ServiceRow,
    client: ClientRow,
}

/// New York, a room and a stylist open on Wednesday 2026-11-04 from 9:00 to 17:00 local time (UTC-5), a haircut of an hour.
async fn salon(pool: PgPool) -> Salon {
    let org = org(&pool, "salon").await;
    sqlx::query("UPDATE organizations SET timezone = 'America/New_York' WHERE id = $1").bind(org).execute(&pool).await.unwrap();
    open_in(&pool, org).await
}

async fn open_in(pool: &PgPool, org: Uuid) -> Salon {
    let room = bookable(pool, org, "room", 1, &[("kind", "room")]).await;
    let stylist = bookable(pool, org, "stylist", 1, &[("role", "stylist")]).await;
    let haircut = service(pool, org, "haircut", 60, Some(is("role", "stylist")), is("kind", "room")).await;
    sqlx::query("UPDATE services SET org_id = $1 WHERE id = $2").bind(org).bind(haircut.id).execute(pool).await.unwrap();
    let client = client(pool).await;
    sqlx::query("UPDATE clients SET org_id = $1 WHERE id = $2").bind(org).bind(client.id).execute(pool).await.unwrap();
    let app = booking_router(pool.clone());
    for bookable in [&room, &stylist] {
        let body = json!({"bookable_id": bookable.id, "date": "2026-11-04", "start_time": "09:00", "end_time": "17:00"});
        assert_eq!(call(&app, "POST", &format!("/api/v1/orgs/{org}/schedules"), Some(body)).await.0, StatusCode::OK);
    }
    Salon { app, org, stylist, haircut, client }
}

impl Salon {
    fn uri(&self, path: &str) -> String {
        format!("/api/v1/orgs/{}/{path}", self.org)
    }

    async fn slots(&self, from: &str, to: &str) -> Vec<Value> {
        let body = json!({"parties": [{"client_id": self.client.id, "services": [{"service_id": self.haircut.id}]}], "start_time": from, "end_time": to});
        let (status, found) = call(&self.app, "POST", &self.uri("availability:search"), Some(body)).await;
        assert_eq!(status, StatusCode::OK, "{found}");
        found["slots"].as_array().unwrap().clone()
    }

    async fn first_slot(&self) -> Value {
        self.slots("2026-11-04T09:00:00", "2026-11-04T12:00:00").await.remove(0)
    }

    fn booking(&self, slot: &Value, request_id: &str) -> Value {
        json!({"state": "booked", "slot": slot, "request_id": request_id, "payer_id": self.client.id})
    }

    async fn book(&self, slot: &Value, request_id: &str) -> Value {
        let (status, appointment) = call(&self.app, "POST", &self.uri("appointments"), Some(self.booking(slot, request_id))).await;
        assert_eq!(status, StatusCode::OK, "{appointment}");
        appointment
    }

    async fn hold(&self, slot: &Value) -> Value {
        let (status, held) = call(&self.app, "POST", &self.uri("appointments"), Some(json!({"state": "held", "slot": slot}))).await;
        assert_eq!(status, StatusCode::OK, "{held}");
        held
    }

    fn path(&self, appointment: &Value) -> String {
        self.uri(&format!("appointments/{}", appointment["id"].as_str().unwrap()))
    }
}

#[sqlx::test]
async fn an_appointment_is_booked_in_one_call_from_a_slot(pool: PgPool) {
    let salon = salon(pool).await;
    let slot = salon.first_slot().await;
    let appointment = salon.book(&slot, "request-1").await;
    assert_eq!(appointment["state"], json!("booked"));
    assert_eq!((appointment["start_time"].clone(), appointment["end_time"].clone()), (json!("2026-11-04T09:00:00-05:00"), json!("2026-11-04T10:00:00-05:00")));
    assert_eq!(appointment["name"], json!(format!("orgs/{}/appointments/{}", salon.org, appointment["id"].as_str().unwrap())));
    assert_eq!(appointment["payer_id"], json!(salon.client.id));
    let line = &appointment["lines"][0];
    assert_eq!((line["client_id"].clone(), line["service_id"].clone(), line["duration"].clone()), (json!(salon.client.id), json!(salon.haircut.id), json!("3600s")));
    assert_eq!(line["price"], json!({"currency_code": "USD", "units": "100", "nanos": 0}));
    assert_eq!(appointment["allocations"].as_array().unwrap().len(), 2);
    assert!(appointment["delete_time"].is_null());
}

#[sqlx::test]
async fn a_booked_slot_is_no_longer_offered(pool: PgPool) {
    let salon = salon(pool).await;
    let slot = salon.first_slot().await;
    salon.book(&slot, "request-1").await;
    let starts: Vec<_> = salon.slots("2026-11-04T09:00:00", "2026-11-04T12:00:00").await.iter().map(|slot| slot["start_time"].clone()).collect();
    assert_eq!(starts, vec![json!("2026-11-04T10:00:00-05:00"), json!("2026-11-04T10:15:00-05:00"), json!("2026-11-04T10:30:00-05:00"), json!("2026-11-04T10:45:00-05:00"), json!("2026-11-04T11:00:00-05:00")]);
}

#[sqlx::test]
async fn the_same_request_id_returns_the_same_appointment(pool: PgPool) {
    let salon = salon(pool.clone()).await;
    let slot = salon.first_slot().await;
    let first = salon.book(&slot, "request-1").await;
    let again = salon.book(&slot, "request-1").await;
    assert_eq!(first["id"], again["id"]);
    let booked: i64 = sqlx::query_scalar("SELECT count(*) FROM appointments").fetch_one(&pool).await.unwrap();
    assert_eq!(booked, 1);
}

#[sqlx::test]
async fn a_held_appointment_blocks_the_slot_and_is_confirmed_under_the_same_id(pool: PgPool) {
    let salon = salon(pool).await;
    let slot = salon.first_slot().await;
    let held = salon.hold(&slot).await;
    assert_eq!(held["state"], json!("held"));
    assert!(held["expire_time"].is_string());
    assert!(salon.slots("2026-11-04T09:00:00", "2026-11-04T10:00:00").await.is_empty());
    let body = json!({"request_id": "confirm-1", "payer_id": salon.client.id, "clients": [salon.client.id], "attributes": {"note": "first visit"}});
    let (status, booked) = call(&salon.app, "POST", &format!("{}:confirm", salon.path(&held)), Some(body.clone())).await;
    assert_eq!(status, StatusCode::OK, "{booked}");
    assert_eq!((booked["id"].clone(), booked["state"].clone(), booked["attributes"].clone()), (held["id"].clone(), json!("booked"), json!({"note": "first visit"})));
    let (_, again) = call(&salon.app, "POST", &format!("{}:confirm", salon.path(&held)), Some(body)).await;
    assert_eq!(again, booked);
    let (_, listed) = call(&salon.app, "GET", &salon.uri("appointments"), None).await;
    assert_eq!(listed["appointments"].as_array().unwrap().len(), 1);
}

#[sqlx::test]
async fn an_appointment_is_read_back_with_its_time(pool: PgPool) {
    let salon = salon(pool).await;
    let booked = salon.book(&salon.first_slot().await, "request-1").await;
    let (status, read) = call(&salon.app, "GET", &salon.path(&booked), None).await;
    assert_eq!((status, &read), (StatusCode::OK, &booked));
    let held = salon.hold(&salon.slots("2026-11-04T12:00:00", "2026-11-04T14:00:00").await[0]).await;
    let (_, read) = call(&salon.app, "GET", &salon.path(&held), None).await;
    assert_eq!(read, held);
}

#[sqlx::test]
async fn cancelling_keeps_the_appointment_as_cancelled_and_frees_the_time(pool: PgPool) {
    let salon = salon(pool).await;
    let slot = salon.first_slot().await;
    let booked = salon.book(&slot, "request-1").await;
    let (status, cancelled) = call(&salon.app, "DELETE", &salon.path(&booked), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(cancelled["state"], json!("cancelled"));
    assert!(cancelled["delete_time"].is_string());
    let (_, listed) = call(&salon.app, "GET", &salon.uri("appointments"), None).await;
    assert!(listed["appointments"].as_array().unwrap().is_empty());
    let filter = "state%20%3D%20%22cancelled%22";
    let (_, shown) = call(&salon.app, "GET", &salon.uri(&format!("appointments?show_deleted=true&filter={filter}")), None).await;
    assert_eq!(shown["appointments"].as_array().unwrap().len(), 1);
    assert!(salon.slots("2026-11-04T09:00:00", "2026-11-04T10:00:00").await.iter().any(|offered| offered["start_time"] == slot["start_time"]));
    assert_eq!(call(&salon.app, "DELETE", &salon.path(&booked), None).await.0, StatusCode::NOT_FOUND);
}

#[sqlx::test]
async fn deleting_a_held_appointment_releases_it(pool: PgPool) {
    let salon = salon(pool).await;
    let held = salon.hold(&salon.first_slot().await).await;
    assert_eq!(call(&salon.app, "DELETE", &salon.path(&held), None).await.0, StatusCode::OK);
    assert!(!salon.slots("2026-11-04T09:00:00", "2026-11-04T10:00:00").await.is_empty());
    assert_eq!(call(&salon.app, "GET", &salon.path(&held), None).await.1["state"], json!("cancelled"));
}

#[sqlx::test]
async fn an_expired_hold_is_gone(pool: PgPool) {
    let salon = salon(pool.clone()).await;
    let held = salon.hold(&salon.first_slot().await).await;
    sqlx::query("UPDATE holds SET expires_at = now() - interval '1 minute'").execute(&pool).await.unwrap();
    assert_eq!(call(&salon.app, "GET", &salon.path(&held), None).await.0, StatusCode::NOT_FOUND);
    let body = json!({"request_id": "late", "payer_id": salon.client.id});
    assert_eq!(call(&salon.app, "POST", &format!("{}:confirm", salon.path(&held)), Some(body)).await.0, StatusCode::NOT_FOUND);
}

#[sqlx::test]
async fn a_slot_the_server_did_not_offer_is_refused(pool: PgPool) {
    let salon = salon(pool.clone()).await;
    let other_org = org(&pool, "other").await;
    let foreign = bookable(&pool, other_org, "foreign room", 1, &[("kind", "room")]).await;
    let slot = salon.first_slot().await;
    let mut other_bookable = slot.clone();
    other_bookable["allocations"][0]["bookable_id"] = json!(foreign.id);
    let mut shorter = slot.clone();
    shorter["allocations"][0]["end_time"] = json!("2026-11-04T09:30:00-05:00");
    shorter["parties"][0]["services"][0]["end_time"] = json!("2026-11-04T09:30:00-05:00");
    let mut late = slot.clone();
    late["allocations"].as_array_mut().unwrap().iter_mut().for_each(|a| {
        a["start_time"] = json!("2026-11-04T19:00:00-05:00");
        a["end_time"] = json!("2026-11-04T20:00:00-05:00");
    });
    late["parties"][0]["services"][0]["start_time"] = json!("2026-11-04T19:00:00-05:00");
    late["parties"][0]["services"][0]["end_time"] = json!("2026-11-04T20:00:00-05:00");
    for claim in [other_bookable, shorter, late] {
        let (status, error) = call(&salon.app, "POST", &salon.uri("appointments"), Some(salon.booking(&claim, "crafted"))).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{claim}");
        assert_eq!(error["error"]["status"], json!("INVALID_ARGUMENT"));
    }
    let booked: i64 = sqlx::query_scalar("SELECT count(*) FROM appointments").fetch_one(&pool).await.unwrap();
    assert_eq!(booked, 0);
}

#[sqlx::test]
async fn a_slot_taken_in_the_meantime_is_refused(pool: PgPool) {
    let salon = salon(pool).await;
    let slot = salon.first_slot().await;
    salon.book(&slot, "request-1").await;
    let (status, _) = call(&salon.app, "POST", &salon.uri("appointments"), Some(salon.booking(&slot, "request-2"))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[sqlx::test]
async fn a_prepaid_method_pays_and_an_empty_one_leaves_the_slot_free(pool: PgPool) {
    let salon = salon(pool.clone()).await;
    let pass = |uses| PaymentMethod {
        org_id: salon.org,
        name: "pass".into(),
        owner_id: Some(salon.client.id),
        eligible: is("mode", "appointment"),
        settlement: Settlement::Prepaid { balance: Amount::Uses(uses), valid: None },
        attributes: serde_json::json!({}),
    };
    let (full, empty) = (PaymentMethodRow::new(PaymentMethodId(Uuid::new_v4()), pass(5)).insert(&pool).await.unwrap(), PaymentMethodRow::new(PaymentMethodId(Uuid::new_v4()), pass(0)).insert(&pool).await.unwrap());
    let slot = salon.first_slot().await;
    let mut body = salon.booking(&slot, "empty");
    body["payments"] = json!([{"method": empty.id, "amount": {"uses": 1}}]);
    let (status, error) = call(&salon.app, "POST", &salon.uri("appointments"), Some(body)).await;
    assert_eq!((status, error["error"]["status"].clone()), (StatusCode::BAD_REQUEST, json!("FAILED_PRECONDITION")));
    assert!(!salon.slots("2026-11-04T09:00:00", "2026-11-04T10:00:00").await.is_empty());
    let mut body = salon.booking(&slot, "full");
    body["payments"] = json!([{"method": full.id, "amount": {"uses": 2}}]);
    let (status, booked) = call(&salon.app, "POST", &salon.uri("appointments"), Some(body)).await;
    assert_eq!(status, StatusCode::OK, "{booked}");
    assert_eq!(booked["payments"], json!([{"method": full.id, "amount": {"uses": 2}}]));
    let settlement: sqlx::types::Json<Settlement> = sqlx::query_scalar("SELECT settlement FROM payment_methods WHERE id = $1").bind(full.id).fetch_one(&pool).await.unwrap();
    assert_eq!(settlement.0, Settlement::Prepaid { balance: Amount::Uses(3), valid: None });
}

/// The ids of the appointments an AIP-160 filter selects.
async fn matching(salon: &Salon, filter: String) -> Vec<Value> {
    let encoded = filter.replace(' ', "%20").replace('"', "%22").replace('=', "%3D").replace('>', "%3E").replace('<', "%3C").replace(':', "%3A");
    let (_, found) = call(&salon.app, "GET", &salon.uri(&format!("appointments?filter={encoded}")), None).await;
    found["appointments"].as_array().unwrap().iter().map(|item| item["id"].clone()).collect()
}

#[sqlx::test]
async fn a_list_is_filtered_by_state_time_and_the_ids_in_it(pool: PgPool) {
    let salon = salon(pool.clone()).await;
    let slots = salon.slots("2026-11-04T09:00:00", "2026-11-04T17:00:00").await;
    let morning = salon.book(&slots[0], "morning").await;
    let held = salon.hold(slots.iter().find(|slot| slot["start_time"] == json!("2026-11-04T13:00:00-05:00")).unwrap()).await;
    assert_eq!(matching(&salon, "state = \"booked\"".into()).await, vec![morning["id"].clone()]);
    assert_eq!(matching(&salon, "state = \"held\"".into()).await, vec![held["id"].clone()]);
    assert_eq!(matching(&salon, format!("client_ids:\"{}\"", salon.client.id.0)).await, vec![morning["id"].clone(), held["id"].clone()]);
    assert_eq!(matching(&salon, format!("bookable_ids:\"{}\" AND state = \"booked\"", salon.stylist.id.0)).await, vec![morning["id"].clone()]);
    assert_eq!(matching(&salon, format!("service_ids:\"{}\"", salon.haircut.id.0)).await.len(), 2);
    assert_eq!(matching(&salon, "start_time >= \"2026-11-04T12:00:00-05:00\"".into()).await, vec![held["id"].clone()]);
    assert_eq!(matching(&salon, "end_time <= \"2026-11-04T11:00:00-05:00\"".into()).await, vec![morning["id"].clone()]);
    assert_eq!(matching(&salon, format!("payer_id = \"{}\"", Uuid::new_v4())).await, Vec::<Value>::new());
    let (_, all) = call(&salon.app, "GET", &salon.uri("appointments"), None).await;
    assert_eq!(all["appointments"].as_array().unwrap().iter().map(|item| item["id"].clone()).collect::<Vec<_>>(), vec![morning["id"].clone(), held["id"].clone()]);
}

#[sqlx::test]
async fn a_brand_sees_the_appointments_of_its_locations_and_not_the_reverse(pool: PgPool) {
    let brand = org(&pool, "brand").await;
    let location = org(&pool, "location").await;
    sqlx::query("UPDATE organizations SET timezone = 'America/New_York' WHERE id = ANY($1)").bind(vec![brand, location]).execute(&pool).await.unwrap();
    group(&pool, Grouped::Org, location, brand).await;
    let salon = open_in(&pool, location).await;
    let booked = salon.book(&salon.first_slot().await, "request-1").await;
    let (_, from_brand) = call(&salon.app, "GET", &format!("/api/v1/orgs/{brand}/appointments"), None).await;
    assert_eq!(from_brand["appointments"].as_array().unwrap().len(), 1);
    let (_, own) = call(&salon.app, "GET", &salon.uri("appointments"), None).await;
    assert_eq!(own["appointments"][0]["id"], booked["id"]);
    let other = org(&pool, "other").await;
    let (_, from_other) = call(&salon.app, "GET", &format!("/api/v1/orgs/{other}/appointments"), None).await;
    assert!(from_other["appointments"].as_array().unwrap().is_empty());
    assert_eq!(call(&salon.app, "GET", &format!("/api/v1/orgs/{other}/appointments/{}", booked["id"].as_str().unwrap()), None).await.0, StatusCode::NOT_FOUND);
}

#[sqlx::test]
async fn a_bad_request_is_an_aip_193_error(pool: PgPool) {
    let salon = salon(pool).await;
    let slot = salon.first_slot().await;
    let cases = [
        (json!({"state": "booked", "slot": slot, "payer_id": salon.client.id}), StatusCode::BAD_REQUEST),
        (json!({"state": "booked", "slot": slot, "request_id": "x"}), StatusCode::BAD_REQUEST),
        (json!({"state": "held", "slot": slot, "payer_id": salon.client.id}), StatusCode::BAD_REQUEST),
        (json!({"state": "held", "slot": slot, "expire_time": "2020-01-01T00:00:00Z"}), StatusCode::BAD_REQUEST),
        (json!({"state": "held", "slot": slot, "expire_time": "2099-01-01T00:00:00Z"}), StatusCode::BAD_REQUEST),
        (json!({"state": "pending", "slot": slot}), StatusCode::BAD_REQUEST),
        (json!({"state": "booked", "slot": slot, "request_id": "x", "payer_id": Uuid::new_v4()}), StatusCode::NOT_FOUND),
    ];
    for (body, expected) in cases {
        let (status, error) = call(&salon.app, "POST", &salon.uri("appointments"), Some(body.clone())).await;
        assert_eq!(status, expected, "{body}");
        assert_eq!(error["error"]["code"], json!(expected.as_u16()));
    }
    assert_eq!(call(&salon.app, "GET", &salon.uri(&format!("appointments/{}", Uuid::new_v4())), None).await.0, StatusCode::NOT_FOUND);
    assert_eq!(call(&salon.app, "GET", &salon.uri("appointments/not-an-id"), None).await.0, StatusCode::BAD_REQUEST);
}
