//! The nine scenarios of docs/TPD-booking.md section 4, each set up and run only through the REST API. Times are UTC;
//! the org is in New York, which is five hours behind on these dates.

mod booking_support;

use axum::http::StatusCode;
use axum::Router;
use booking_support::on_spot;
use booking_support::rest::*;
use megh::booking::{booking_router, PaymentMethodRow};
use serde_json::{json, Value};
use sqlx::PgPool;
use uuid::Uuid;

/// An org in New York with the REST router.
struct Place {
    app: Router,
    org: Uuid,
    on_spot: PaymentMethodRow,
}

fn need(filter: &str, engagement: &str) -> Value {
    json!({"who": {"Filter": filter}, "count": 1, "engagement": engagement, "role": "provider"})
}

fn room(filter: &str) -> Value {
    json!({"who": {"Filter": filter}, "count": 1, "engagement": "engaged", "role": "room"})
}

fn step(duration: &str, needs: Vec<Value>) -> Value {
    json!({"duration": duration, "client": "present", "needs": needs, "attributes": {}})
}

/// A window of 2026 in UTC, such as `at("11-04", "14:00")`.
fn at(day: &str, time: &str) -> String {
    format!("2026-{day}T{time}:00Z")
}

/// The New York day that starts on 2026-`day` (five hours after midnight UTC) as a window.
fn local_day(day: u32) -> (String, String) {
    (at(&format!("11-{day:02}"), "05:00"), at(&format!("11-{:02}", day + 1), "05:00"))
}

impl Place {
    async fn new(pool: PgPool) -> Self {
        let org = clinic(&pool).await;
        let on_spot = on_spot(&pool, org).await;
        Self { app: booking_router(pool), org, on_spot }
    }

    async fn bookable(&self, name: &str, capacity: u32, tags: Value) -> Value {
        rest_bookable(&self.app, self.org, name, capacity, tags).await
    }

    async fn client(&self, name: &str, tags: Value) -> Value {
        rest_client(&self.app, self.org, name, tags).await
    }

    async fn service(&self, name: &str, mode: &str, units: i64, steps: Vec<Value>, attributes: Value) -> Value {
        let body = json!({"org_id": self.org, "name": name, "mode": mode, "price": {"micros": units * 1_000_000, "currency": "USD"}, "steps": steps, "variants": [], "attributes": attributes});
        created(&self.app, self.org, "services", body).await
    }

    async fn appointment_service(&self, name: &str, steps: Vec<Value>) -> Value {
        self.service(name, "appointment", 50, steps, json!({})).await
    }

    /// Open for the bookable at the local time (`20261104T090000`) for the span, once or on the weekdays in `rule`.
    async fn open(&self, bookable: &Value, local: &str, span: &str, rule: &str) {
        let recurrence = format!("DTSTART;TZID=America/New_York:{local}\nRRULE:{rule}");
        let body = json!({"org_id": self.org, "service_id": null, "bookable_id": bookable["id"], "kind": "open", "recurrence": recurrence, "span": span, "reason": null, "notes": null});
        created(&self.app, self.org, "schedules", body).await;
    }

    async fn open_on(&self, bookable: &Value, local: &str, span: &str) {
        self.open(bookable, local, span, "FREQ=DAILY;COUNT=1").await;
    }

    /// The request of a search, and the slots found.
    async fn search(&self, parties: Value, from: &str, to: &str) -> (Value, Vec<Value>) {
        let request = json!({"parties": parties, "within": {"start": from, "end": to}});
        let (status, found) = call(&self.app, "POST", &format!("/api/v1/orgs/{}/availability:search", self.org), Some(request.clone())).await;
        assert_eq!(status, StatusCode::OK, "{found}");
        (request, found["slots"].as_array().unwrap().clone())
    }

    async fn one(&self, service: &Value, client: Option<&Value>, from: &str, to: &str) -> (Value, Vec<Value>) {
        let party = json!([{"client": client.map(|c| c["id"].clone()), "services": [{"service": service["id"]}]}]);
        self.search(party, from, to).await
    }

    /// The first slot found, with the request that found it.
    async fn first(&self, service: &Value, client: &Value, from: &str, to: &str) -> (Value, Value) {
        let (request, mut slots) = self.one(service, Some(client), from, to).await;
        (request, slots.remove(0))
    }

    /// Books the slot, a payment for each placement: on the spot, or with the pass.
    async fn book(&self, found: &(Value, Value), key: &str, payer: &Value, pass: Option<&Value>) -> (StatusCode, Value) {
        let (request, slot) = found;
        let payment = match pass {
            Some(pass) => rest_payment(payer, pass, json!({"type": "uses", "value": 1})),
            None => rest_payment(payer, &json!({"id": self.on_spot.id}), json!({"type": "uses", "value": 0})),
        };
        let payments = vec![payment; slot["placements"].as_array().unwrap().len()];
        let body = json!({"request": request, "slot": slot, "payments": payments, "request_id": key});
        call(&self.app, "POST", &format!("/api/v1/orgs/{}/appointments", self.org), Some(body)).await
    }
}

/// When each slot starts: its first placement.
fn starts(slots: &[Value]) -> Vec<String> {
    slots.iter().map(|slot| slot["placements"][0]["during"]["start"].as_str().unwrap().to_string()).collect()
}

fn allocated(slot: &Value, bookable: &Value) -> Vec<Value> {
    slot["allocations"].as_array().unwrap().iter().filter(|a| a["bookable"] == bookable["id"]).cloned().collect()
}

// a) A hair colour: the stylist applies it, moves to another client while it dries, and comes back to finish.
#[sqlx::test]
async fn a_hair_colour_frees_the_stylist_while_it_dries(pool: PgPool) {
    let p = Place::new(pool).await;
    let (stylist, chair1, chair2) = (p.bookable("Mia", 1, json!({"role": "stylist"})).await, p.bookable("Chair 1", 1, json!({"kind": "chair"})).await, p.bookable("Chair 2", 1, json!({"kind": "chair"})).await);
    for bookable in [&stylist, &chair1, &chair2] {
        p.open_on(bookable, "20261104T090000", "PT8H").await;
    }
    let who = "role = \"stylist\"";
    let colour = p.appointment_service("Hair colour", vec![
        step("PT10M", vec![need(who, "engaged"), room("kind = \"chair\"")]),
        step("PT30M", vec![need(who, "free"), room("kind = \"chair\"")]),
        step("PT20M", vec![need(who, "engaged"), room("kind = \"chair\"")]),
    ]).await;
    let cut = p.appointment_service("Cut", vec![step("PT10M", vec![need(who, "engaged"), room("kind = \"chair\"")])]).await;
    let (ann, ben) = (p.client("Ann", json!({})).await, p.client("Ben", json!({})).await);
    let first = p.first(&colour, &ann, &at("11-04", "14:00"), &at("11-04", "15:00")).await;
    assert_eq!(p.book(&first, "ann", &ann, None).await.0, StatusCode::OK);
    let during_the_wait = p.one(&cut, Some(&ben), &at("11-04", "14:10"), &at("11-04", "14:20")).await.1;
    assert_eq!(starts(&during_the_wait), vec![at("11-04", "14:10")]);
    assert!(!allocated(&during_the_wait[0], &stylist).is_empty() && !allocated(&during_the_wait[0], &chair2).is_empty());
    let at_the_start = p.one(&cut, Some(&ben), &at("11-04", "14:00"), &at("11-04", "14:10")).await.1;
    assert!(at_the_start.is_empty(), "the stylist is busy preparing the first colour");
}

// b) A dentist and a hygienist on one patient: the hygienist sets up, the dentist does the procedures, the hygienist cleans.
#[sqlx::test]
async fn a_dentist_and_a_hygienist_take_turns_on_one_patient(pool: PgPool) {
    let p = Place::new(pool).await;
    let (dentist, hygienist, operatory) = (p.bookable("Dr Albert", 1, json!({"role": "dentist"})).await, p.bookable("Beth", 1, json!({"role": "hygienist"})).await, p.bookable("Operatory 1", 1, json!({"kind": "operatory"})).await);
    for bookable in [&dentist, &hygienist, &operatory] {
        p.open_on(bookable, "20261104T090000", "PT8H").await;
    }
    let mut setup = step("PT15M", vec![need("role = \"hygienist\"", "engaged"), room("kind = \"operatory\"")]);
    setup["attributes"] = json!({"code": "setup"});
    let mut work = step("PT30M", vec![need("role = \"dentist\"", "engaged"), room("kind = \"operatory\"")]);
    work["attributes"] = json!({"codes": ["D2391", "D2392"]});
    let clean = step("PT20M", vec![need("role = \"hygienist\"", "engaged"), room("kind = \"operatory\"")]);
    let procedure = p.appointment_service("Filling", vec![setup, work, clean]).await;
    let patient = p.client("Pat", json!({})).await;
    let found = p.first(&procedure, &patient, &at("11-04", "14:00"), &at("11-04", "16:00")).await;
    let turns = |who: &Value| allocated(&found.1, who).iter().map(|a| (a["during"]["start"].as_str().unwrap()[11..16].to_string(), a["during"]["end"].as_str().unwrap()[11..16].to_string())).collect::<Vec<_>>();
    assert_eq!(turns(&hygienist), vec![("14:00".into(), "14:15".into()), ("14:45".into(), "15:05".into())]);
    assert_eq!(turns(&dentist), vec![("14:15".into(), "14:45".into())]);
    assert_eq!(p.book(&found, "pat", &patient, None).await.0, StatusCode::OK);
}

// c) A gym: a client comes in, works out on their own, leaves; until the floor is full. No provider.
#[sqlx::test]
async fn a_gym_takes_clients_until_the_floor_is_full(pool: PgPool) {
    let p = Place::new(pool).await;
    let floor = p.bookable("Floor", 3, json!({"kind": "floor"})).await;
    p.open_on(&floor, "20261104T060000", "PT16H").await;
    let workout = p.appointment_service("Workout", vec![step("PT1H", vec![room("kind = \"floor\"")])]).await;
    let members = [p.client("m1", json!({})).await, p.client("m2", json!({})).await, p.client("m3", json!({})).await, p.client("m4", json!({})).await];
    for (index, member) in members.iter().take(3).enumerate() {
        let found = p.first(&workout, member, &at("11-04", "11:00"), &at("11-04", "12:00")).await;
        assert_eq!(p.book(&found, &format!("m{index}"), member, None).await.0, StatusCode::OK);
    }
    let full = p.one(&workout, Some(&members[3]), &at("11-04", "11:00"), &at("11-04", "12:00")).await.1;
    assert!(full.is_empty());
    let later = p.one(&workout, Some(&members[3]), &at("11-04", "11:00"), &at("11-04", "13:00")).await.1;
    assert_eq!(starts(&later).first(), Some(&at("11-04", "12:00")));
}

// d) A one-time talk with a capacity: people book up to it.
#[sqlx::test]
async fn a_talk_takes_bookings_up_to_its_capacity(pool: PgPool) {
    let p = Place::new(pool).await;
    let hall = p.bookable("Hall", 2, json!({"kind": "hall"})).await;
    p.open_on(&hall, "20261112T180000", "PT2H").await;
    let talk = p.appointment_service("TED talk", vec![step("PT2H", vec![room("kind = \"hall\"")])]).await;
    let guests = [p.client("g1", json!({})).await, p.client("g2", json!({})).await, p.client("g3", json!({})).await];
    let day = local_day(12);
    let mut booked = vec![];
    for (index, guest) in guests.iter().take(2).enumerate() {
        let found = p.first(&talk, guest, &day.0, &day.1).await;
        let (status, appointments) = p.book(&found, &format!("g{index}"), guest, None).await;
        assert_eq!(status, StatusCode::OK);
        booked.push(appointments);
    }
    assert!(p.one(&talk, Some(&guests[2]), &day.0, &day.1).await.1.is_empty());
    let (status, _) = call(&p.app, "DELETE", &format!("/api/v1/orgs/{}/appointments/{}", p.org, booked[0]["data"][0]["id"].as_str().unwrap()), None).await;
    assert_eq!(status, StatusCode::OK);
    let freed = p.one(&talk, Some(&guests[2]), &day.0, &day.1).await.1;
    assert_eq!(starts(&freed), vec![at("11-12", "23:00")]);
}

// e) A restaurant: four guests book a table of four, one appointment each; a waiter may be assigned, with no time of his own blocked.
#[sqlx::test]
async fn a_table_is_booked_for_a_party_and_a_waiter_is_only_on_demand(pool: PgPool) {
    let p = Place::new(pool).await;
    let (table1, table2, waiter) = (p.bookable("Table 1", 4, json!({"kind": "table"})).await, p.bookable("Table 2", 4, json!({"kind": "table"})).await, p.bookable("Wes", 1, json!({"role": "waiter"})).await);
    for bookable in [&table1, &table2] {
        p.open_on(bookable, "20261104T180000", "PT5H").await;
    }
    let dinner = p.appointment_service("Dinner", vec![step("PT90M", vec![room("kind = \"table\""), need("role = \"waiter\"", "on_demand")])]).await;
    let group = |name: &'static str| {
        let (app, org) = (p.app.clone(), p.org);
        async move {
            let mut guests = vec![];
            for index in 0..4 {
                guests.push(rest_client(&app, org, &format!("{name}{index}"), json!({})).await);
            }
            guests
        }
    };
    let (anns, bens, cys) = (group("Ann").await, group("Ben").await, group("Cy").await);
    let party = |guests: &Vec<Value>| Value::Array(guests.iter().map(|guest| json!({"client": guest["id"], "services": [{"service": dinner["id"]}]})).collect());
    let day = (at("11-05", "00:00"), at("11-05", "01:30"));
    let (request, mut slots) = p.search(party(&anns), &day.0, &day.1).await;
    let slot_ann = slots.remove(0);
    assert!(allocated(&slot_ann, &waiter).iter().all(|allocation| allocation["engagement"] == json!("on_demand")));
    let (status, booked) = p.book(&(request, slot_ann), "ann", &anns[0], None).await;
    assert_eq!((status, booked["data"].as_array().map(Vec::len)), (StatusCode::OK, Some(4)), "{booked}");
    let (request, mut slots) = p.search(party(&bens), &day.0, &day.1).await;
    assert_eq!(p.book(&(request, slots.remove(0)), "ben", &bens[0], None).await.0, StatusCode::OK);
    assert!(p.search(party(&cys), &day.0, &day.1).await.1.is_empty(), "both tables are taken");
}

// f) A recurring class every Thursday and Sunday 18:00 to 19:00, paid with a pass of 30 visits valid for a period.
#[sqlx::test]
async fn a_recurring_class_is_booked_with_a_pass_valid_for_a_period(pool: PgPool) {
    let p = Place::new(pool).await;
    let studio = p.bookable("Studio", 12, json!({"kind": "studio"})).await;
    p.open(&studio, "20261105T180000", "PT1H", "FREQ=WEEKLY;BYDAY=TH,SU").await;
    let class = p.service("Music class", "class", 20, vec![step("PT1H", vec![room("kind = \"studio\"")])], json!({"category": "music"})).await;
    let learner = p.client("Lea", json!({})).await;
    let valid = json!({"start": "2026-11-01T04:00:00Z", "end": "2027-02-01T05:00:00Z"});
    let pass = json!({"org_id": p.org, "name": "30 classes", "owner_id": learner["id"], "eligible": {"Filter": "attributes.category = \"music\""}, "settlement": {"type": "prepaid", "value": {"balance": {"type": "uses", "value": 30}, "valid": valid}}, "attributes": {}});
    let pass = created(&p.app, p.org, "paymentMethods", pass).await;
    let (request, week) = p.one(&class, Some(&learner), &at("11-05", "00:00"), &at("11-12", "00:00")).await;
    assert_eq!(starts(&week), vec![at("11-05", "23:00"), at("11-08", "23:00")]);
    for (index, slot) in week.iter().enumerate() {
        assert_eq!(p.book(&(request.clone(), slot.clone()), &format!("class-{index}"), &learner, Some(&pass)).await.0, StatusCode::OK);
    }
    let (_, after) = call(&p.app, "GET", &path(p.org, "paymentMethods", &pass), None).await;
    assert_eq!(after["settlement"]["value"]["balance"], json!({"type": "uses", "value": 28}));
    let (request, late) = p.one(&class, Some(&learner), "2027-02-04T00:00:00Z", "2027-02-05T00:00:00Z").await; // a Thursday, after the pass's last day
    assert_eq!(late.len(), 1, "the class goes on after the pass has run out");
    let (status, error) = p.book(&(request, late[0].clone()), "late", &learner, Some(&pass)).await;
    assert_eq!((status, error["error"]["status"].clone()), (StatusCode::BAD_REQUEST, json!("FAILED_PRECONDITION")));
}

// g) Pet grooming: the pet is the client, its owner pays.
#[sqlx::test]
async fn a_pet_is_groomed_and_its_owner_pays(pool: PgPool) {
    let p = Place::new(pool).await;
    let (groomer, table) = (p.bookable("Gina", 1, json!({"role": "groomer"})).await, p.bookable("Grooming table", 1, json!({"kind": "grooming"})).await);
    for bookable in [&groomer, &table] {
        p.open_on(bookable, "20261104T090000", "PT8H").await;
    }
    let grooming = p.appointment_service("Grooming", vec![step("PT1H", vec![need("role = \"groomer\"", "engaged"), room("kind = \"grooming\"")])]).await;
    let (rex, owner) = (p.client("Rex", json!({"kind": "dog", "breed": "beagle"})).await, p.client("Sam", json!({"kind": "person"})).await);
    let found = p.first(&grooming, &rex, &at("11-04", "14:00"), &at("11-04", "15:00")).await;
    let (status, booked) = p.book(&found, "rex", &owner, None).await;
    assert_eq!(status, StatusCode::OK, "{booked}");
    let appointment = &booked["data"][0];
    assert_eq!((appointment["payment"]["payer_id"].clone(), appointment["client_id"].clone()), (owner["id"].clone(), rex["id"].clone()));
}

// h) A couples massage: two people at once in one room, each with a therapist.
#[sqlx::test]
async fn a_couple_is_massaged_at_the_same_time_in_one_room(pool: PgPool) {
    let p = Place::new(pool).await;
    let (suite, tess, tom) = (p.bookable("Couples suite", 2, json!({"kind": "suite"})).await, p.bookable("Tess", 1, json!({"role": "therapist"})).await, p.bookable("Tom", 1, json!({"role": "therapist"})).await);
    for bookable in [&suite, &tess, &tom] {
        p.open_on(bookable, "20261104T100000", "PT8H").await;
    }
    let massage = p.appointment_service("Massage", vec![step("PT1H", vec![need("role = \"therapist\"", "engaged"), room("kind = \"suite\"")])]).await;
    let (ann, ben) = (p.client("Ann", json!({})).await, p.client("Ben", json!({})).await);
    let couple = json!([{"client": ann["id"], "services": [{"service": massage["id"]}]}, {"client": ben["id"], "services": [{"service": massage["id"]}]}]);
    let (request, mut slots) = p.search(couple, &at("11-04", "15:00"), &at("11-04", "16:00")).await;
    let slot = slots.remove(0);
    assert_eq!(slot["placements"][0]["during"], slot["placements"][1]["during"]);
    assert_eq!(allocated(&slot, &suite).len(), 2);
    let (status, booked) = p.book(&(request, slot), "couple", &ann, None).await;
    assert_eq!(status, StatusCode::OK, "{booked}");
    let clients: Vec<Value> = booked["data"].as_array().unwrap().iter().map(|appointment| appointment["client_id"].clone()).collect();
    assert_eq!(clients.len(), 2);
    assert!(clients.contains(&ann["id"]) && clients.contains(&ben["id"]));
}

// i) A float pod: a room, no provider.
#[sqlx::test]
async fn a_float_pod_is_booked_without_a_provider(pool: PgPool) {
    let p = Place::new(pool).await;
    let (pod1, pod2) = (p.bookable("Pod 1", 1, json!({"kind": "pod"})).await, p.bookable("Pod 2", 1, json!({"kind": "pod"})).await);
    for pod in [&pod1, &pod2] {
        p.open_on(pod, "20261104T090000", "PT12H").await;
    }
    let float = p.service("Float", "resource", 80, vec![step("PT1H", vec![room("kind = \"pod\"")])], json!({})).await;
    let guests = [p.client("a", json!({})).await, p.client("b", json!({})).await, p.client("c", json!({})).await];
    for (index, guest) in guests.iter().take(2).enumerate() {
        let found = p.first(&float, guest, &at("11-04", "14:00"), &at("11-04", "15:00")).await;
        assert_eq!(p.book(&found, &format!("pod-{index}"), guest, None).await.0, StatusCode::OK);
    }
    assert!(p.one(&float, Some(&guests[2]), &at("11-04", "14:00"), &at("11-04", "15:00")).await.1.is_empty());
    assert_eq!(starts(&p.one(&float, Some(&guests[2]), &at("11-04", "14:00"), &at("11-04", "16:00")).await.1).first(), Some(&at("11-04", "15:00")));
}
