//! The nine scenarios of docs/TPD-booking.md section 4, each set up and run only through the REST API.

mod booking_support;

use axum::http::StatusCode;
use axum::Router;
use booking_support::rest::*;
use megh::booking::booking_router;
use serde_json::{json, Value};
use sqlx::PgPool;
use uuid::Uuid;

fn money(units: &str) -> Value {
    json!({"currency_code": "USD", "units": units, "nanos": 0})
}

/// An org in New York with the REST router.
struct Place {
    app: Router,
    org: Uuid,
}

impl Place {
    async fn new(pool: PgPool) -> Self {
        let org = clinic(&pool).await;
        Self { app: booking_router(pool), org }
    }

    async fn make(&self, collection: &str, body: Value) -> Value {
        created(&self.app, self.org, collection, body).await
    }

    async fn bookable(&self, name: &str, capacity: u32, tags: Value) -> Value {
        self.make("bookables", json!({"org_id": self.org, "name": name, "capacity": capacity, "attributes": tags})).await
    }

    async fn client(&self, name: &str, tags: Value) -> Value {
        self.make("clients", json!({"org_id": self.org, "name": name, "first_name": name, "last_name": "Client", "attributes": tags})).await
    }

    async fn service(&self, name: &str, steps: Value) -> Value {
        self.make("services", json!({"display_name": name, "price": money("50"), "steps": steps})).await
    }

    /// Open from `from` to `to` local time on the date, for a bookable.
    async fn open_on(&self, bookable: &Value, date: &str, from: &str, to: &str) {
        self.make("schedules", json!({"bookable_id": bookable["id"], "date": date, "start_time": from, "end_time": to})).await;
    }

    async fn search(&self, parties: Value, from: &str, to: &str) -> Vec<Value> {
        let body = json!({"parties": parties, "start_time": from, "end_time": to});
        let (status, found) = call(&self.app, "POST", &format!("/api/v1/orgs/{}/availability:search", self.org), Some(body)).await;
        assert_eq!(status, StatusCode::OK, "{found}");
        found["slots"].as_array().unwrap().clone()
    }

    async fn one(&self, service: &Value, client: Option<&Value>, from: &str, to: &str) -> Vec<Value> {
        let party = json!([{"client_id": client.map(|c| c["id"].clone()), "services": [{"service_id": service["id"]}]}]);
        self.search(party, from, to).await
    }

    async fn book(&self, slot: &Value, key: &str, payer: &Value, extra: Value) -> (StatusCode, Value) {
        let mut body = json!({"state": "booked", "slot": slot, "request_id": key, "payer_id": payer["id"]});
        extra.as_object().unwrap().iter().for_each(|(name, value)| body[name] = value.clone());
        call(&self.app, "POST", &format!("/api/v1/orgs/{}/appointments", self.org), Some(body)).await
    }
}

fn starts(slots: &[Value]) -> Vec<String> {
    slots.iter().map(|slot| slot["start_time"].as_str().unwrap().to_string()).collect()
}

fn allocated(slot: &Value, bookable: &Value) -> Vec<Value> {
    slot["allocations"].as_array().unwrap().iter().filter(|a| a["bookable_id"] == bookable["id"]).cloned().collect()
}

// a) A hair colour: the stylist applies it, moves to another client while it dries, and comes back to finish.
#[sqlx::test]
async fn a_hair_colour_frees_the_stylist_while_it_dries(pool: PgPool) {
    let p = Place::new(pool).await;
    let (stylist, chair1, chair2) = (p.bookable("Mia", 1, json!({"role": "stylist"})).await, p.bookable("Chair 1", 1, json!({"kind": "chair"})).await, p.bookable("Chair 2", 1, json!({"kind": "chair"})).await);
    for bookable in [&stylist, &chair1, &chair2] {
        p.open_on(bookable, "2026-11-04", "09:00", "17:00").await;
    }
    let who = "role = \"stylist\"";
    let colour = p.service("Hair colour", json!([
        {"duration": "600s", "needs": [{"who": who}, {"who": "kind = \"chair\""}]},
        {"duration": "1800s", "needs": [{"who": who, "engagement": "free"}, {"who": "kind = \"chair\""}]},
        {"duration": "1200s", "needs": [{"who": who}, {"who": "kind = \"chair\""}]}
    ])).await;
    let cut = p.service("Cut", json!([{"duration": "600s", "needs": [{"who": who}, {"who": "kind = \"chair\""}]}])).await;
    let (ann, ben) = (p.client("Ann", json!({})).await, p.client("Ben", json!({})).await);
    let first = p.one(&colour, Some(&ann), "2026-11-04T09:00:00", "2026-11-04T10:00:00").await.remove(0);
    assert_eq!(p.book(&first, "ann", &ann, json!({})).await.0, StatusCode::OK);
    let during_the_wait = p.one(&cut, Some(&ben), "2026-11-04T09:10:00", "2026-11-04T09:20:00").await;
    assert_eq!(starts(&during_the_wait), vec!["2026-11-04T09:10:00-05:00"]);
    assert!(!allocated(&during_the_wait[0], &stylist).is_empty() && !allocated(&during_the_wait[0], &chair2).is_empty());
    let at_the_start = p.one(&cut, Some(&ben), "2026-11-04T09:00:00", "2026-11-04T09:10:00").await;
    assert!(at_the_start.is_empty(), "the stylist is busy preparing the first colour");
}

// b) A dentist and a hygienist on one patient: the hygienist sets up, the dentist does the procedures, the hygienist cleans.
#[sqlx::test]
async fn a_dentist_and_a_hygienist_take_turns_on_one_patient(pool: PgPool) {
    let p = Place::new(pool).await;
    let (dentist, hygienist, room) = (p.bookable("Dr Albert", 1, json!({"role": "dentist"})).await, p.bookable("Beth", 1, json!({"role": "hygienist"})).await, p.bookable("Operatory 1", 1, json!({"kind": "operatory"})).await);
    for bookable in [&dentist, &hygienist, &room] {
        p.open_on(bookable, "2026-11-04", "09:00", "17:00").await;
    }
    let room_need = json!({"who": "kind = \"operatory\""});
    let procedure = p.service("Filling", json!([
        {"duration": "900s", "needs": [{"who": "role = \"hygienist\""}, room_need.clone()], "attributes": {"code": "setup"}},
        {"duration": "1800s", "needs": [{"who": "role = \"dentist\""}, room_need.clone()], "attributes": {"codes": ["D2391", "D2392"]}},
        {"duration": "1200s", "needs": [{"who": "role = \"hygienist\""}, room_need]}
    ])).await;
    let patient = p.client("Pat", json!({})).await;
    let slot = p.one(&procedure, Some(&patient), "2026-11-04T09:00:00", "2026-11-04T11:00:00").await.remove(0);
    let turns = |who: &Value| allocated(&slot, who).iter().map(|a| (a["start_time"].as_str().unwrap()[11..16].to_string(), a["end_time"].as_str().unwrap()[11..16].to_string())).collect::<Vec<_>>();
    assert_eq!(turns(&hygienist), vec![("09:00".into(), "09:15".into()), ("09:45".into(), "10:05".into())]);
    assert_eq!(turns(&dentist), vec![("09:15".into(), "09:45".into())]);
    assert_eq!(p.book(&slot, "pat", &patient, json!({})).await.0, StatusCode::OK);
}

// c) A gym: a client comes in, works out on their own, leaves; until the floor is full. No provider.
#[sqlx::test]
async fn a_gym_takes_clients_until_the_floor_is_full(pool: PgPool) {
    let p = Place::new(pool).await;
    let floor = p.bookable("Floor", 3, json!({"kind": "floor"})).await;
    p.open_on(&floor, "2026-11-04", "06:00", "22:00").await;
    let workout = p.service("Workout", json!([{"duration": "3600s", "needs": [{"who": "kind = \"floor\""}]}])).await;
    let members = [p.client("m1", json!({})).await, p.client("m2", json!({})).await, p.client("m3", json!({})).await, p.client("m4", json!({})).await];
    for (index, member) in members.iter().take(3).enumerate() {
        let slot = p.one(&workout, Some(member), "2026-11-04T06:00:00", "2026-11-04T07:00:00").await.remove(0);
        assert_eq!(p.book(&slot, &format!("m{index}"), member, json!({})).await.0, StatusCode::OK);
    }
    let full = p.one(&workout, Some(&members[3]), "2026-11-04T06:00:00", "2026-11-04T07:00:00").await;
    assert!(full.is_empty());
    let later = p.one(&workout, Some(&members[3]), "2026-11-04T06:00:00", "2026-11-04T08:00:00").await;
    assert_eq!(starts(&later).first().map(String::as_str), Some("2026-11-04T07:00:00-05:00"));
}

// d) A one-time talk with a capacity: people book up to it, then wait.
#[sqlx::test]
async fn a_talk_takes_bookings_up_to_its_capacity_and_then_a_waitlist(pool: PgPool) {
    let p = Place::new(pool).await;
    let hall = p.bookable("Hall", 2, json!({"kind": "hall"})).await;
    p.open_on(&hall, "2026-11-12", "18:00", "20:00").await;
    let talk = p.service("TED talk", json!([{"duration": "7200s", "needs": [{"who": "kind = \"hall\""}]}])).await;
    let guests = [p.client("g1", json!({})).await, p.client("g2", json!({})).await, p.client("g3", json!({})).await];
    let mut booked = vec![];
    for (index, guest) in guests.iter().take(2).enumerate() {
        let slot = p.one(&talk, Some(guest), "2026-11-12T00:00:00", "2026-11-13T00:00:00").await.remove(0);
        let (status, appointment) = p.book(&slot, &format!("g{index}"), guest, json!({})).await;
        assert_eq!(status, StatusCode::OK);
        booked.push(appointment);
    }
    assert!(p.one(&talk, Some(&guests[2]), "2026-11-12T00:00:00", "2026-11-13T00:00:00").await.is_empty());
    let wait = json!({"client_id": guests[2]["id"], "target": {"window": {"service_id": talk["id"], "start_time": "2026-11-12T18:00:00", "end_time": "2026-11-12T20:00:00"}}});
    p.make("waitlistEntries", wait).await;
    let (status, _) = call(&p.app, "DELETE", &format!("/api/v1/orgs/{}/appointments/{}", p.org, booked[0]["id"].as_str().unwrap()), None).await;
    assert_eq!(status, StatusCode::OK);
    let freed = p.one(&talk, Some(&guests[2]), "2026-11-12T00:00:00", "2026-11-13T00:00:00").await;
    assert_eq!(starts(&freed), vec!["2026-11-12T18:00:00-05:00"]);
    let waiting = ids(&p.app, p.org, "waitlistEntries", "").await;
    assert_eq!(waiting.len(), 1);
}

// e) A restaurant: n guests book a table of n; a waiter may be assigned, with no time of his own blocked.
#[sqlx::test]
async fn a_table_is_booked_for_a_party_and_a_waiter_is_only_on_demand(pool: PgPool) {
    let p = Place::new(pool).await;
    let (table1, table2, waiter) = (p.bookable("Table 1", 4, json!({"kind": "table"})).await, p.bookable("Table 2", 4, json!({"kind": "table"})).await, p.bookable("Wes", 1, json!({"role": "waiter"})).await);
    for bookable in [&table1, &table2] {
        p.open_on(bookable, "2026-11-04", "18:00", "23:00").await;
    }
    let dinner = p.service("Dinner", json!([{"duration": "5400s", "needs": [{"who": "kind = \"table\""}, {"who": "role = \"waiter\"", "engagement": "on_demand"}]}])).await;
    let (ann, ben) = (p.client("Ann", json!({})).await, p.client("Ben", json!({})).await);
    let party = |client: &Value| json!([{"client_id": client["id"], "seats": 4, "services": [{"service_id": dinner["id"]}]}]);
    let slot_ann = p.search(party(&ann), "2026-11-04T19:00:00", "2026-11-04T20:30:00").await.remove(0);
    assert_eq!(allocated(&slot_ann, &waiter)[0]["engagement"], json!("on_demand"));
    assert_eq!(p.book(&slot_ann, "ann", &ann, json!({})).await.0, StatusCode::OK);
    let slot_ben = p.search(party(&ben), "2026-11-04T19:00:00", "2026-11-04T20:30:00").await.remove(0);
    assert_eq!(allocated(&slot_ben, &waiter)[0]["engagement"], json!("on_demand"));
    assert_eq!(p.book(&slot_ben, "ben", &ben, json!({})).await.0, StatusCode::OK);
    let third = p.client("Cy", json!({})).await;
    assert!(p.search(party(&third), "2026-11-04T19:00:00", "2026-11-04T20:30:00").await.is_empty(), "both tables are taken");
}

// f) A recurring class every Thursday and Sunday 18:00 to 19:00, paid with a pass of 30 visits valid for a period.
#[sqlx::test]
async fn a_recurring_class_is_booked_with_a_pass_valid_for_a_period(pool: PgPool) {
    let p = Place::new(pool).await;
    let studio = p.bookable("Studio", 12, json!({"kind": "studio"})).await;
    p.make("schedules", json!({"bookable_id": studio["id"], "days": ["TH", "SU"], "start_time": "18:00", "end_time": "19:00", "start_date": "2026-11-05"})).await;
    let class = p.make("services", json!({"display_name": "Music class", "mode": "class", "price": money("20"), "attributes": {"category": "music"}, "steps": [{"duration": "3600s", "needs": [{"who": "kind = \"studio\""}]}]})).await;
    let learner = p.client("Lea", json!({})).await;
    let pass = p.make("paymentMethods", json!({"display_name": "30 classes", "owner_id": learner["id"], "eligible": "attributes.category = \"music\"", "settlement": {"prepaid": {"balance": {"uses": 30}, "valid": {"start_time": "2026-11-01T00:00:00", "end_time": "2027-02-01T00:00:00"}}}})).await;
    let week = p.one(&class, Some(&learner), "2026-11-05T00:00:00", "2026-11-12T00:00:00").await;
    assert_eq!(starts(&week), vec!["2026-11-05T18:00:00-05:00", "2026-11-08T18:00:00-05:00"]);
    for (index, slot) in week.iter().enumerate() {
        let payment = json!({"payments": [{"method": pass["id"], "amount": {"uses": 1}}]});
        assert_eq!(p.book(slot, &format!("class-{index}"), &learner, payment).await.0, StatusCode::OK);
    }
    let (_, after) = call(&p.app, "GET", &path(p.org, "paymentMethods", &pass), None).await;
    assert_eq!(after["settlement"]["prepaid"]["balance"], json!({"uses": 28}));
    let december = p.one(&class, Some(&learner), "2027-02-04T00:00:00", "2027-02-05T00:00:00").await; // a Thursday, after the pass's last day
    assert_eq!(december.len(), 1, "the class goes on after the pass has run out");
    let payment = json!({"payments": [{"method": pass["id"], "amount": {"uses": 1}}]});
    let (status, error) = p.book(&december[0], "late", &learner, payment).await;
    assert_eq!((status, error["error"]["status"].clone()), (StatusCode::BAD_REQUEST, json!("FAILED_PRECONDITION")));
}

// g) Pet grooming: the pet is the client, its owner pays.
#[sqlx::test]
async fn a_pet_is_groomed_and_its_owner_pays(pool: PgPool) {
    let p = Place::new(pool).await;
    let (groomer, table) = (p.bookable("Gina", 1, json!({"role": "groomer"})).await, p.bookable("Grooming table", 1, json!({"kind": "grooming"})).await);
    for bookable in [&groomer, &table] {
        p.open_on(bookable, "2026-11-04", "09:00", "17:00").await;
    }
    let grooming = p.service("Grooming", json!([{"duration": "3600s", "needs": [{"who": "role = \"groomer\""}, {"who": "kind = \"grooming\""}]}])).await;
    let (rex, owner) = (p.client("Rex", json!({"kind": "dog", "breed": "beagle"})).await, p.client("Sam", json!({"kind": "person"})).await);
    let slot = p.one(&grooming, Some(&rex), "2026-11-04T09:00:00", "2026-11-04T10:00:00").await.remove(0);
    let (status, appointment) = p.book(&slot, "rex", &owner, json!({"clients": [rex["id"]]})).await;
    assert_eq!(status, StatusCode::OK, "{appointment}");
    assert_eq!((appointment["payer_id"].clone(), appointment["lines"][0]["client_id"].clone()), (owner["id"].clone(), rex["id"].clone()));
}

// h) A couples massage: two people at once in one room, each with a therapist.
#[sqlx::test]
async fn a_couple_is_massaged_at_the_same_time_in_one_room(pool: PgPool) {
    let p = Place::new(pool).await;
    let (suite, tess, tom) = (p.bookable("Couples suite", 2, json!({"kind": "suite"})).await, p.bookable("Tess", 1, json!({"role": "therapist"})).await, p.bookable("Tom", 1, json!({"role": "therapist"})).await);
    for bookable in [&suite, &tess, &tom] {
        p.open_on(bookable, "2026-11-04", "10:00", "18:00").await;
    }
    let massage = p.service("Massage", json!([{"duration": "3600s", "needs": [{"who": "role = \"therapist\""}, {"who": "kind = \"suite\""}]}])).await;
    let (ann, ben) = (p.client("Ann", json!({})).await, p.client("Ben", json!({})).await);
    let couple = json!([{"client_id": ann["id"], "services": [{"service_id": massage["id"]}]}, {"client_id": ben["id"], "services": [{"service_id": massage["id"]}]}]);
    let slot = p.search(couple, "2026-11-04T10:00:00", "2026-11-04T11:00:00").await.remove(0);
    assert_eq!(slot["parties"][0]["services"][0]["start_time"], slot["parties"][1]["services"][0]["start_time"]);
    assert_eq!(allocated(&slot, &suite).iter().map(|a| a["seats"].as_u64().unwrap()).sum::<u64>(), 2);
    let (status, appointment) = p.book(&slot, "couple", &ann, json!({"clients": [ann["id"], ben["id"]]})).await;
    assert_eq!(status, StatusCode::OK, "{appointment}");
    assert_eq!(appointment["client_ids"].as_array().unwrap().len(), 2);
}

// i) A float pod: a room, no provider.
#[sqlx::test]
async fn a_float_pod_is_booked_without_a_provider(pool: PgPool) {
    let p = Place::new(pool).await;
    let (pod1, pod2) = (p.bookable("Pod 1", 1, json!({"kind": "pod"})).await, p.bookable("Pod 2", 1, json!({"kind": "pod"})).await);
    for pod in [&pod1, &pod2] {
        p.open_on(pod, "2026-11-04", "09:00", "21:00").await;
    }
    let float = p.make("services", json!({"display_name": "Float", "mode": "resource", "price": money("80"), "steps": [{"duration": "3600s", "needs": [{"who": "kind = \"pod\""}]}]})).await;
    let guests = [p.client("a", json!({})).await, p.client("b", json!({})).await, p.client("c", json!({})).await];
    for (index, guest) in guests.iter().take(2).enumerate() {
        let slot = p.one(&float, Some(guest), "2026-11-04T09:00:00", "2026-11-04T10:00:00").await.remove(0);
        assert_eq!(p.book(&slot, &format!("pod-{index}"), guest, json!({})).await.0, StatusCode::OK);
    }
    assert!(p.one(&float, Some(&guests[2]), "2026-11-04T09:00:00", "2026-11-04T10:00:00").await.is_empty());
    assert_eq!(starts(&p.one(&float, Some(&guests[2]), "2026-11-04T09:00:00", "2026-11-04T11:00:00").await).first().map(String::as_str), Some("2026-11-04T10:00:00-05:00"));
}
