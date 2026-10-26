//! B9 (docs/TPD-booking.md): open slots. A slot is when every party's client, providers and room are free:
//! inside a schedule, outside blockouts, with capacity (or only a waitlist place) left by stored reservations,
//! one row per place taken.
//! A step's provider group matches a provider's role; `providers` and `resources` match fields, not attributes.
//! Booking (`POST /appointments`) stores an appointment only when the same check passes, and stores its time.

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

const MON_WED: &str =
    "DTSTART;TZID=Asia/Kolkata:20261023T000000\nRRULE:FREQ=WEEKLY;BYDAY=MO,WE;UNTIL=20261230T235959Z";
const MONDAY: &str = "2026-10-26";

fn app(pool: PgPool, grants: &[&str]) -> Router {
    let (api, _) = megh::booking::router().split_for_parts();
    Router::new()
        .nest("/api/v1", api.layer(from_fn(authorizer)))
        .layer(Extension(grants.iter().map(|grant| Grant::new(*grant)).collect::<Vec<_>>()))
        .with_state(AppContext { pool })
}

async fn call(app: &Router, method: &str, uri: &str, body: Value) -> (StatusCode, Value) {
    let request = Request::builder()
        .method(method)
        .uri(uri)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let service = MapRequestLayer::new(megh::booking::rewrite_aip_136).layer(app.clone());
    let response = service.oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
}

/// A clinic open Monday and Wednesday 9:00–16:00 India time, built through the API.
struct Clinic {
    app: Router,
    pool: PgPool,
    tenant: Uuid,
    room: String,
    meera: String,
    albert: String,
    raj: String,
    extraction: String,
    float: String,
    pods: Vec<String>,
    ada: String,
}

impl Clinic {
    async fn new(pool: PgPool) -> Self {
        megh::migrate(&pool).await.unwrap();
        let tenant: Uuid = sqlx::query_scalar("INSERT INTO tenants (name) VALUES ('Clinic') RETURNING id")
            .fetch_one(&pool)
            .await
            .unwrap();
        let app = app(pool.clone(), &["*"]);
        let base = format!("/api/v1/tenants/{tenant}");
        let post = |path: &str, body: Value| {
            let (app, uri) = (app.clone(), format!("{base}/{path}"));
            async move {
                let (status, saved) = call(&app, "POST", &uri, body).await;
                assert_eq!(status, StatusCode::OK, "{saved}");
                saved["id"].as_str().unwrap().to_string()
            }
        };
        let person = |name: &str, role: &str| json!({"first_name": name, "last_name": "X", "name": name, "role": role});
        let room = post("resources", json!({"name": "Room 1", "role": "operatory"})).await;
        let meera = post("providers", person("Meera", "hygienist")).await;
        let albert = post("providers", person("Albert", "dentist")).await;
        let raj = post("providers", person("Raj", "dentist")).await;
        let extraction = post("services", json!({
            "name": "Tooth extraction", "setup": "PT10M", "cleanup": "PT10M",
            "steps": [
                {"duration": "PT15M", "provider1": "hygienist"},
                {"duration": "PT45M", "provider1": "hygienist", "provider2": "dentist"},
                {"duration": "PT15M", "provider1": "hygienist"}
            ]
        }))
        .await;
        let float = post("services", json!({"name": "Float", "steps": [{"duration": "PT1H"}]})).await;
        let mut pods = Vec::new();
        for name in ["Pod 1", "Pod 2"] {
            pods.push(post("resources", json!({"name": name, "role": "pod", "waitlist": 1})).await);
        }
        let ada = post("clients", json!({"first_name": "Ada", "last_name": "L", "name": "Ada"})).await;
        post("schedules", json!({"name": "Clinic hours", "recurrence": MON_WED, "starts_at": "09:00:00", "ends_at": "16:00:00"})).await;
        Clinic { app, pool, tenant, room, meera, albert, raj, extraction, float, pods, ada }
    }

    async fn search(&self, body: Value) -> (StatusCode, Value) {
        call(&self.app, "POST", &format!("/api/v1/tenants/{}/appointments:available", self.tenant), body).await
    }

    async fn post(&self, path: &str, body: Value) {
        let (status, saved) = call(&self.app, "POST", &format!("/api/v1/tenants/{}/{path}", self.tenant), body).await;
        assert_eq!(status, StatusCode::OK, "{saved}");
    }

    /// Time a stored appointment in `status` takes from `participant`, as booking stores it: one reservation
    /// row per place, none once cancelled.
    async fn occupy(&self, status: &str, participant: &str, from: &str, to: &str) {
        let appointment: Uuid = sqlx::query_scalar(
            "INSERT INTO appointments (tenant_id, id, client_id, service_id, resource_id, starts_at, ends_at, status, payment) \
             VALUES ($1, $2, $3::uuid, $4::uuid, $5::uuid, $6::timestamptz, $7::timestamptz, $8, '{\"payer_id\": \"00000000-0000-0000-0000-000000000000\", \"method\": \"card\"}') RETURNING id",
        )
        .bind(self.tenant)
        .bind(Uuid::now_v7())
        .bind(&self.ada)
        .bind(&self.float)
        .bind(&self.room)
        .bind(from)
        .bind(to)
        .bind(status)
        .fetch_one(&self.pool)
        .await
        .unwrap();
        if status == "cancelled" {
            return;
        }
        sqlx::query(
            "INSERT INTO reservations (tenant_id, id, owner_id, participant_id, during, waitlist) \
             VALUES ($1, gen_random_uuid(), $2, $3::uuid, tstzrange($4::timestamptz, $5::timestamptz), $6)",
        )
        .bind(self.tenant)
        .bind(appointment)
        .bind(participant)
        .bind(from)
        .bind(to)
        .bind(status == "waitlist")
        .execute(&self.pool)
        .await
        .unwrap();
    }
}

fn at(time: &str) -> String {
    format!("{MONDAY}T{time}:00+05:30")
}

fn starts(found: &Value, party: usize) -> Vec<String> {
    found["slots"].as_array().unwrap().iter().map(|slot| slot["parties"][party]["starts_at"].as_str().unwrap().to_string()).collect()
}

#[sqlx::test(migrations = false)]
async fn dental_slots_take_a_hygienist_a_dentist_and_the_room(pool: PgPool) {
    let clinic = Clinic::new(pool).await;
    let window = json!({"from": at("09:00"), "to": at("12:00")});
    let party = |extra: Value| {
        let mut body = window.clone();
        let mut party = json!({"client": {"id": clinic.ada}, "service_id": clinic.extraction});
        party.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        body["parties"] = json!([party]);
        body
    };

    let (status, found) = clinic.search(party(json!({"resources": [{"role": "operatory"}]}))).await;
    assert_eq!(status, StatusCode::OK, "{found}");
    assert_eq!(starts(&found, 0), [at("09:10"), at("10:45")], "setup from 9:00, one 95 minute room time apart, inside the window");
    assert_eq!(
        found["slots"][0]["parties"][0],
        json!({
            "client": {"id": clinic.ada}, "service_id": clinic.extraction,
            "providers": [{"id": clinic.albert}, {"id": clinic.meera}], "resources": [{"id": clinic.room}],
            "starts_at": at("09:10"), "ends_at": at("10:25"), "waitlist": false, "capacity": 1
        }),
        "one provider per role: dentist, then hygienist"
    );
    assert_eq!(found["next_available_day"], Value::Null);

    let (_, found) = clinic.search(party(json!({"providers": [{"id": clinic.raj}], "resources": [{"role": "operatory"}]}))).await;
    assert_eq!(found["slots"][0]["parties"][0]["providers"], json!([{"id": clinic.raj}, {"id": clinic.meera}]));

    clinic.occupy("booked", &clinic.albert, &at("09:00"), &at("09:25")).await;
    let (_, found) = clinic.search(party(json!({"providers": [{"name": "Albert"}], "resources": [{"role": "operatory"}]}))).await;
    assert_eq!(starts(&found, 0), [at("09:10"), at("10:45")], "the dentist is needed only from 9:25, in the second step");
    clinic.occupy("booked", &clinic.albert, &at("09:00"), &at("09:30")).await;
    let (_, found) = clinic.search(party(json!({"providers": [{"name": "Albert"}], "resources": [{"role": "operatory"}]}))).await;
    assert_eq!(starts(&found, 0), [at("10:45")]);

    let (_, found) = clinic.search(party(json!({"providers": [{"name": "Nobody"}]}))).await;
    assert_eq!((found["slots"].clone(), found["next_available_day"].clone()), (json!([]), Value::Null));

    for not_a_field in [json!({"providers": [{"gender": "female"}]}), json!({"resources": [{"attributes": {}}]})] {
        assert_eq!(clinic.search(party(not_a_field.clone())).await.0, StatusCode::BAD_REQUEST, "{not_a_field}");
    }
}

#[sqlx::test(migrations = false)]
async fn a_known_client_is_not_offered_time_already_taken(pool: PgPool) {
    let clinic = Clinic::new(pool).await;
    clinic.occupy("booked", &clinic.ada, &at("09:00"), &at("10:00")).await;
    let search = |client: Value| {
        json!({"from": at("09:00"), "to": at("12:00"), "parties": [{"client": client, "service_id": clinic.float, "resources": [{"name": "Pod 1"}]}]})
    };
    let (_, found) = clinic.search(search(json!({"id": clinic.ada}))).await;
    assert_eq!(starts(&found, 0), [at("10:00"), at("11:00")]);
    let (_, found) = clinic.search(search(json!({"tag": "guest"}))).await;
    assert_eq!(starts(&found, 0), [at("09:00"), at("10:00"), at("11:00")]);
}

#[sqlx::test(migrations = false)]
async fn a_full_room_offers_its_waitlist_and_cancelled_time_is_free(pool: PgPool) {
    let clinic = Clinic::new(pool).await;
    let pod = &clinic.pods[0];
    let search = json!({"from": at("09:00"), "to": at("11:00"),
        "parties": [{"client": {"tag": "guest"}, "service_id": clinic.float, "resources": [{"id": pod}]}]});
    clinic.occupy("booked", pod, &at("09:00"), &at("10:00")).await;
    clinic.occupy("cancelled", pod, &at("10:00"), &at("11:00")).await;
    let (_, found) = clinic.search(search.clone()).await;
    let waitlist = |found: &Value| found["slots"].as_array().unwrap().iter().map(|slot| slot["parties"][0]["waitlist"].clone()).collect::<Vec<_>>();
    assert_eq!((starts(&found, 0), waitlist(&found)), (vec![at("09:00"), at("10:00")], vec![json!(true), json!(false)]));

    clinic.occupy("waitlist", pod, &at("09:00"), &at("10:00")).await;
    let (_, found) = clinic.search(search).await;
    assert_eq!(starts(&found, 0), [at("10:00")], "capacity 1 and the one waitlist place are taken");
}

#[sqlx::test(migrations = false)]
async fn blockouts_close_schedules(pool: PgPool) {
    let clinic = Clinic::new(pool).await;
    clinic.post("blockouts", json!({"name": "Lunch", "recurrence": MON_WED, "starts_at": "12:00:00", "ends_at": "13:00:00", "reason": "lunch"})).await;
    clinic.post("blockouts", json!({"name": "Pod 1 repair", "resource_id": clinic.pods[0], "recurrence": MON_WED, "starts_at": "09:00:00", "ends_at": "10:00:00"})).await;
    let search = |pod: &str| {
        json!({"from": at("09:00"), "to": at("14:00"), "parties": [{"client": {"tag": "guest"}, "service_id": clinic.float, "resources": [{"id": pod}]}]})
    };
    let (_, found) = clinic.search(search(&clinic.pods[0])).await;
    assert_eq!(starts(&found, 0), [at("10:00"), at("11:00"), at("13:00")]);
    let (_, found) = clinic.search(search(&clinic.pods[1])).await;
    assert_eq!(starts(&found, 0), [at("09:00"), at("10:00"), at("11:00"), at("13:00")]);
}

#[sqlx::test(migrations = false)]
async fn parties_are_placed_together_within_the_gaps(pool: PgPool) {
    let clinic = Clinic::new(pool).await;
    let floats = |parties: Vec<(Value, Value)>, gaps: Value| {
        let parties: Vec<Value> = parties
            .into_iter()
            .map(|(client, resources)| json!({"client": client, "service_id": clinic.float, "resources": resources}))
            .collect();
        let mut body = json!({"from": at("09:00"), "to": at("11:00"), "parties": parties});
        body.as_object_mut().unwrap().extend(gaps.as_object().unwrap().clone());
        body
    };
    let any_pod = json!([{"role": "pod"}]);
    let (_, couple) = clinic.search(floats(vec![(json!({"tag": "a"}), any_pod.clone()), (json!({"tag": "b"}), any_pod.clone())], json!({}))).await;
    let first = &couple["slots"][0]["parties"];
    assert_eq!((first[0]["starts_at"].clone(), first[1]["starts_at"].clone()), (json!(at("09:00")), json!(at("09:00"))));
    assert_eq!((first[0]["resources"].clone(), first[1]["resources"].clone()), (json!([{"id": clinic.pods[0]}]), json!([{"id": clinic.pods[1]}])));

    let (_, twice) = clinic.search(floats(vec![(json!({"id": clinic.ada}), any_pod.clone()), (json!({"id": clinic.ada}), any_pod.clone())], json!({}))).await;
    assert_eq!((starts(&twice, 0), starts(&twice, 1)), (vec![at("09:00")], vec![at("10:00")]), "back to back for one client");

    let one_pod = json!([{"id": clinic.pods[0]}]);
    let together = vec![(json!({"tag": "a"}), one_pod.clone()), (json!({"tag": "b"}), one_pod.clone())];
    let (_, waiting) = clinic.search(floats(together.clone(), json!({}))).await;
    let second = &waiting["slots"][0]["parties"][1];
    assert_eq!(
        (second["starts_at"].clone(), second["waitlist"].clone()),
        (json!(at("09:00")), json!(true)),
        "one pod takes one at once; the other gets its waitlist place"
    );
    let (_, staggered) = clinic.search(floats(together, json!({"client_gap": "PT1H"}))).await;
    let first = &staggered["slots"][0]["parties"];
    assert_eq!(
        (first[0]["starts_at"].clone(), first[1]["starts_at"].clone(), first[1]["waitlist"].clone()),
        (json!(at("09:00")), json!(at("10:00")), json!(false)),
        "within the gap, a free place comes before a waitlist place"
    );
}

#[sqlx::test(migrations = false)]
async fn a_day_without_slots_names_the_next_one(pool: PgPool) {
    let clinic = Clinic::new(pool).await;
    let tuesday = json!({"from": "2026-10-27T09:00:00+05:30", "to": "2026-10-27T18:00:00+05:30",
        "parties": [{"client": {"tag": "guest"}, "service_id": clinic.float}]});
    let (status, found) = clinic.search(tuesday).await;
    assert_eq!((status, found["slots"].clone(), found["next_available_day"].clone()), (StatusCode::OK, json!([]), json!("2026-10-28")));
}

#[sqlx::test(migrations = false)]
async fn bad_searches_are_refused(pool: PgPool) {
    let clinic = Clinic::new(pool.clone()).await;
    let party = json!({"client": {"tag": "guest"}, "service_id": clinic.float});
    for bad in [
        json!({"from": at("12:00"), "to": at("09:00"), "parties": [party]}),
        json!({"from": at("09:00"), "to": at("12:00"), "parties": []}),
        json!({"from": at("09:00"), "to": at("12:00"), "parties": [{"client": {"tag": "guest"}, "service_id": Uuid::now_v7()}]}),
        json!({"from": at("09:00"), "to": at("12:00"), "parties": [{"client": {}, "service_id": clinic.float}]}),
    ] {
        assert_eq!(clinic.search(bad.clone()).await.0, StatusCode::BAD_REQUEST, "{bad}");
    }
    let reader = app(pool, &[&format!("tenants:read:{}", clinic.tenant), "appointments:read"]);
    let uri = format!("/api/v1/tenants/{}/appointments:available", clinic.tenant);
    let body = json!({"from": at("09:00"), "to": at("12:00"), "parties": [party]});
    assert_eq!(call(&reader, "POST", &uri, body).await.0, StatusCode::FORBIDDEN, "needs appointments:available");
}

impl Clinic {
    async fn book(&self, body: Value) -> (StatusCode, Value) {
        call(&self.app, "POST", &format!("/api/v1/tenants/{}/appointments", self.tenant), body).await
    }

    fn appointment(&self, service: &str, room: &str, providers: &[&str], from: &str, to: &str, status: &str) -> Value {
        json!({
            "client_id": self.ada, "service_id": service, "resource_id": room, "provider_ids": providers,
            "starts_at": at(from), "ends_at": at(to), "status": status,
            "payment": {"payer_id": self.ada, "method": "card"}
        })
    }
}

#[sqlx::test(migrations = false)]
async fn booking_stores_an_open_slot_and_takes_its_time(pool: PgPool) {
    let clinic = Clinic::new(pool).await;
    let extraction = clinic.appointment(&clinic.extraction, &clinic.room, &[&clinic.meera, &clinic.albert], "09:10", "10:25", "booked");
    let (status, booked) = clinic.book(extraction.clone()).await;
    assert_eq!((status, booked["status"].clone()), (StatusCode::OK, json!("booked")), "{booked}");
    assert!(booked["booked_at"].as_str().is_some_and(|at| !at.starts_with("1970")), "set by the server");
    let stretches: i64 = sqlx::query_scalar("SELECT count(*) FROM reservations WHERE owner_id = $1::uuid")
        .bind(booked["id"].as_str().unwrap())
        .fetch_one(&clinic.pool)
        .await
        .unwrap();
    assert_eq!(stretches, 6, "the client, the room, the dentist's step and the hygienist's three steps");

    assert_eq!(clinic.book(extraction).await.0, StatusCode::CONFLICT, "the room and the providers are taken");
    let search = json!({"from": at("09:00"), "to": at("12:00"),
        "parties": [{"client": {"tag": "guest"}, "service_id": clinic.extraction, "resources": [{"role": "operatory"}]}]});
    let (_, found) = clinic.search(search).await;
    assert_eq!(starts(&found, 0), [at("10:45")], "the next room time; the room is taken until 10:35, its cleanup");

    let raj = clinic.appointment(&clinic.extraction, &clinic.room, &[&clinic.meera, &clinic.raj], "10:45", "12:00", "booked");
    for (bad, why) in [
        (clinic.appointment(&clinic.extraction, &clinic.room, &[&clinic.meera, &clinic.raj], "10:45", "11:30", "booked"), "ends_at is not the service's time"),
        (clinic.appointment(&clinic.extraction, &clinic.room, &[&clinic.albert, &clinic.raj], "10:45", "12:00", "booked"), "no hygienist"),
        (clinic.appointment(&clinic.extraction, &clinic.room, &[&clinic.meera], "10:45", "12:00", "booked"), "no dentist"),
        (clinic.appointment(&clinic.extraction, &clinic.room, &[&clinic.meera, &clinic.raj], "10:45", "12:00", "confirmed"), "new is booked or waitlist"),
    ] {
        assert_eq!(clinic.book(bad).await.0, StatusCode::BAD_REQUEST, "{why}");
    }
    assert_eq!(clinic.book(raj).await.0, StatusCode::OK);

    let tuesday = json!({
        "client_id": clinic.ada, "service_id": clinic.float, "resource_id": clinic.pods[0],
        "starts_at": "2026-10-27T09:00:00+05:30", "ends_at": "2026-10-27T10:00:00+05:30", "status": "booked",
        "payment": {"payer_id": clinic.ada, "method": "card"}
    });
    assert_eq!(clinic.book(tuesday).await.0, StatusCode::CONFLICT, "outside the schedule");
}

#[sqlx::test(migrations = false)]
async fn booking_takes_capacity_then_the_waitlist_and_cancel_frees_it(pool: PgPool) {
    let clinic = Clinic::new(pool).await;
    let pod = &clinic.pods[0];
    let float = |status: &str| clinic.appointment(&clinic.float, pod, &[], "09:00", "10:00", status);
    let other_client = |status: &str| {
        let mut body = float(status);
        body["client_id"] = json!(clinic.ada);
        body
    };
    let (status, first) = clinic.book(float("booked")).await;
    assert_eq!(status, StatusCode::OK, "{first}");
    assert_eq!(clinic.book(other_client("booked")).await.0, StatusCode::CONFLICT, "Ada is already booked then");

    let guest: String = {
        let (status, saved) = call(&clinic.app, "POST", &format!("/api/v1/tenants/{}/clients", clinic.tenant), json!({"first_name": "Bo", "last_name": "B", "name": "Bo"})).await;
        assert_eq!(status, StatusCode::OK);
        saved["id"].as_str().unwrap().to_string()
    };
    let for_guest = |status: &str| {
        let mut body = float(status);
        body["client_id"] = json!(guest);
        body
    };
    assert_eq!(clinic.book(for_guest("booked")).await.0, StatusCode::CONFLICT, "capacity 1 is taken");
    let (status, waiting) = clinic.book(for_guest("waitlist")).await;
    assert_eq!((status, waiting["status"].clone()), (StatusCode::OK, json!("waitlist")), "{waiting}");
    let to_book = format!("/api/v1/tenants/{}/appointments/{}:book", clinic.tenant, waiting["id"].as_str().unwrap());
    assert_eq!(call(&clinic.app, "POST", &to_book, json!({})).await.0, StatusCode::CONFLICT, ":book needs a free place");

    let item = format!("/api/v1/tenants/{}/appointments/{}", clinic.tenant, first["id"].as_str().unwrap());
    assert_eq!(call(&clinic.app, "DELETE", &item, json!({})).await.0, StatusCode::OK);
    let (status, booked) = call(&clinic.app, "POST", &to_book, json!({})).await;
    assert_eq!((status, booked["status"].clone()), (StatusCode::OK, json!("booked")), "cancelling freed the place");
    assert_eq!(clinic.book(float("booked")).await.0, StatusCode::CONFLICT, "the waitlisted guest took it");
}

#[sqlx::test(migrations = false)]
async fn two_bookings_at_once_cannot_take_one_place(pool: PgPool) {
    let clinic = Clinic::new(pool).await;
    let mut bodies = Vec::new();
    for name in ["Cy", "Di"] {
        let (_, client) = call(&clinic.app, "POST", &format!("/api/v1/tenants/{}/clients", clinic.tenant), json!({"first_name": name, "last_name": "X", "name": name})).await;
        let mut body = clinic.appointment(&clinic.float, &clinic.pods[1], &[], "09:00", "10:00", "booked");
        body["client_id"] = client["id"].clone();
        bodies.push(body);
    }
    let (first, second) = tokio::join!(clinic.book(bodies[0].clone()), clinic.book(bodies[1].clone()));
    let mut statuses = [first.0, second.0];
    statuses.sort();
    assert_eq!(statuses, [StatusCode::OK, StatusCode::CONFLICT]);
}

#[sqlx::test(migrations = false)]
async fn patch_moves_an_appointment_only_to_an_open_time(pool: PgPool) {
    let clinic = Clinic::new(pool).await;
    let (pod1, pod2) = (&clinic.pods[0], &clinic.pods[1]);
    let (_, ada) = clinic.book(clinic.appointment(&clinic.float, pod1, &[], "09:00", "10:00", "booked")).await;
    let (_, bo) = call(&clinic.app, "POST", &format!("/api/v1/tenants/{}/clients", clinic.tenant), json!({"first_name": "Bo", "last_name": "B", "name": "Bo"})).await;
    let mut other = clinic.appointment(&clinic.float, pod1, &[], "11:00", "12:00", "booked");
    other["client_id"] = bo["id"].clone();
    assert_eq!(clinic.book(other).await.0, StatusCode::OK);
    let item = format!("/api/v1/tenants/{}/appointments/{}", clinic.tenant, ada["id"].as_str().unwrap());
    let patch = |body: Value| call(&clinic.app, "PATCH", &item, body);

    let (status, moved) = patch(json!({"starts_at": at("10:00"), "ends_at": at("11:00")})).await;
    assert_eq!((status, moved["starts_at"].clone()), (StatusCode::OK, json!("2026-10-26T04:30:00Z")), "{moved}");
    let search = json!({"from": at("09:00"), "to": at("10:00"),
        "parties": [{"client": {"tag": "guest"}, "service_id": clinic.float, "resources": [{"id": pod1}]}]});
    let (_, found) = clinic.search(search).await;
    assert_eq!(found["slots"][0]["parties"][0]["waitlist"], json!(false), "9:00 was given back");

    assert_eq!(patch(json!({"starts_at": at("11:00"), "ends_at": at("12:00")})).await.0, StatusCode::CONFLICT, "Bo has the pod");
    let (status, moved) = patch(json!({"starts_at": at("11:00"), "ends_at": at("12:00"), "resource_id": pod2})).await;
    assert_eq!((status, moved["resource_id"].clone()), (StatusCode::OK, json!(pod2)), "{moved}");
    assert_eq!(
        patch(json!({"starts_at": "2026-10-27T09:00:00+05:30", "ends_at": "2026-10-27T10:00:00+05:30"})).await.0,
        StatusCode::CONFLICT,
        "outside the schedule"
    );
    assert_eq!(patch(json!({"starts_at": at("13:00")})).await.0, StatusCode::BAD_REQUEST, "ends_at must follow");
    assert_eq!(patch(json!({"client_id": bo["id"]})).await.0, StatusCode::BAD_REQUEST);
    assert_eq!(patch(json!({"service_id": clinic.extraction})).await.0, StatusCode::BAD_REQUEST);

    assert_eq!(call(&clinic.app, "DELETE", &item, json!({})).await.0, StatusCode::OK);
    assert_eq!(patch(json!({"starts_at": at("13:00"), "ends_at": at("14:00")})).await.0, StatusCode::BAD_REQUEST, "a cancelled appointment does not move");
}

#[sqlx::test(migrations = false)]
async fn each_booking_takes_one_place_and_cancel_gives_it_back(pool: PgPool) {
    let clinic = Clinic::new(pool).await;
    let (_, studio) = call(&clinic.app, "POST", &format!("/api/v1/tenants/{}/resources", clinic.tenant), json!({"name": "Studio", "capacity": 2, "waitlist": 1})).await;
    let studio = studio["id"].as_str().unwrap().to_string();
    let search = json!({"from": at("09:00"), "to": at("10:00"),
        "parties": [{"client": {"tag": "guest"}, "service_id": clinic.float, "resources": [{"id": studio}]}]});
    let left = |found: &Value| {
        let party = &found["slots"][0]["parties"][0];
        (party["capacity"].clone(), party["waitlist"].clone())
    };
    let mut clients = Vec::new();
    for name in ["Cy", "Di", "Ed"] {
        let (_, client) = call(&clinic.app, "POST", &format!("/api/v1/tenants/{}/clients", clinic.tenant), json!({"first_name": name, "last_name": "X", "name": name})).await;
        clients.push(client["id"].clone());
    }
    let class = |client: &Value, status: &str| {
        let mut body = clinic.appointment(&clinic.float, &studio, &[], "09:00", "10:00", status);
        body["client_id"] = client.clone();
        body
    };

    assert_eq!(left(&clinic.search(search.clone()).await.1), (json!(2), json!(false)));
    let (_, first) = clinic.book(class(&clients[0], "booked")).await;
    assert_eq!(left(&clinic.search(search.clone()).await.1), (json!(1), json!(false)));
    assert_eq!(clinic.book(class(&clients[1], "booked")).await.0, StatusCode::OK);
    assert_eq!(left(&clinic.search(search.clone()).await.1), (json!(0), json!(true)), "full; the waitlist place is left");
    assert_eq!(clinic.book(class(&clients[2], "waitlist")).await.0, StatusCode::OK);
    assert_eq!(clinic.search(search.clone()).await.1["slots"], json!([]), "capacity and waitlist both taken");

    let item = format!("/api/v1/tenants/{}/appointments/{}", clinic.tenant, first["id"].as_str().unwrap());
    assert_eq!(call(&clinic.app, "DELETE", &item, json!({})).await.0, StatusCode::OK);
    assert_eq!(left(&clinic.search(search).await.1), (json!(1), json!(false)), "the cancel gave one place back");
}

impl Clinic {
    async fn hold(&self, slot: &Value) -> (StatusCode, Value) {
        call(&self.app, "POST", &format!("/api/v1/tenants/{}/reservations", self.tenant), slot.clone()).await
    }

    async fn first_slot(&self, parties: Value) -> Value {
        let (_, found) = self.search(json!({"from": at("09:00"), "to": at("12:00"), "parties": parties})).await;
        found["slots"][0].clone()
    }
}

#[sqlx::test(migrations = false)]
async fn a_hold_keeps_a_slot_until_it_is_booked_released_or_expired(pool: PgPool) {
    let clinic = Clinic::new(pool).await;
    let party = json!([{"client": {"id": clinic.ada}, "service_id": clinic.float, "resources": [{"id": clinic.room}]}]);
    let slot = clinic.first_slot(party.clone()).await;
    assert_eq!(slot["parties"][0]["starts_at"], json!(at("09:00")));

    let (status, held) = clinic.hold(&slot).await;
    assert_eq!(status, StatusCode::OK, "{held}");
    assert_eq!(clinic.first_slot(party.clone()).await["parties"][0]["starts_at"], json!(at("10:00")), "9:00 is held");
    let booking = clinic.appointment(&clinic.float, &clinic.room, &[], "09:00", "10:00", "booked");
    assert_eq!(clinic.book(booking.clone()).await.0, StatusCode::CONFLICT, "only through the hold");

    let mut elsewhere = clinic.appointment(&clinic.float, &clinic.room, &[], "10:00", "11:00", "booked");
    elsewhere["reservation_id"] = held["id"].clone();
    assert_eq!(clinic.book(elsewhere).await.0, StatusCode::CONFLICT, "not the held time");
    let mut from_hold = booking.clone();
    from_hold["reservation_id"] = held["id"].clone();
    let (status, booked) = clinic.book(from_hold.clone()).await;
    assert_eq!(status, StatusCode::OK, "{booked}");
    let release = format!("/api/v1/tenants/{}/reservations/{}", clinic.tenant, held["id"].as_str().unwrap());
    assert_eq!(call(&clinic.app, "DELETE", &release, json!({})).await.0, StatusCode::NOT_FOUND, "the booking took its places");
    assert_eq!(clinic.book(from_hold).await.0, StatusCode::CONFLICT);

    let slot = clinic.first_slot(party.clone()).await;
    let (_, held) = clinic.hold(&slot).await;
    let release = format!("/api/v1/tenants/{}/reservations/{}", clinic.tenant, held["id"].as_str().unwrap());
    let (status, released) = call(&clinic.app, "DELETE", &release, json!({})).await;
    assert_eq!((status, released["places"].clone()), (StatusCode::OK, json!(2)), "the room and Ada");
    assert_eq!(clinic.first_slot(party.clone()).await["parties"][0]["starts_at"], json!(at("10:00")), "released");

    let (_, held) = clinic.hold(&slot).await;
    sqlx::query("UPDATE reservations SET expires_at = now() - interval '1 minute' WHERE owner_id = $1::uuid")
        .bind(held["id"].as_str().unwrap())
        .execute(&clinic.pool)
        .await
        .unwrap();
    assert_eq!(clinic.first_slot(party).await["parties"][0]["starts_at"], json!(at("10:00")), "an expired hold takes nothing");
    let mut late = clinic.appointment(&clinic.float, &clinic.room, &[], "10:00", "11:00", "booked");
    late["reservation_id"] = held["id"].clone();
    assert_eq!(clinic.book(late).await.0, StatusCode::CONFLICT, "expired");

    let purge = format!("/api/v1/tenants/{}/reservations:purge", clinic.tenant);
    let (status, purged) = call(&clinic.app, "POST", &purge, json!({"before": "2026-10-26T05:00:00Z"})).await;
    assert_eq!((status, purged["places"].clone()), (StatusCode::OK, json!(4)), "the expired hold's two places and the 9:00 booking's two");
}

#[sqlx::test(migrations = false)]
async fn a_hold_of_two_parties_is_booked_one_appointment_at_a_time(pool: PgPool) {
    let clinic = Clinic::new(pool).await;
    let (_, bo) = call(&clinic.app, "POST", &format!("/api/v1/tenants/{}/clients", clinic.tenant), json!({"first_name": "Bo", "last_name": "B", "name": "Bo"})).await;
    let couple = json!([
        {"client": {"id": clinic.ada}, "service_id": clinic.float, "resources": [{"role": "pod"}]},
        {"client": {"id": bo["id"]}, "service_id": clinic.float, "resources": [{"role": "pod"}]}
    ]);
    let slot = clinic.first_slot(couple.clone()).await;
    let (status, held) = clinic.hold(&slot).await;
    assert_eq!(status, StatusCode::OK, "{held}");
    let booking = |party: &Value, client: &Value| {
        json!({
            "client_id": client, "service_id": clinic.float, "resource_id": party["resources"][0]["id"],
            "starts_at": party["starts_at"], "ends_at": party["ends_at"], "status": "booked",
            "payment": {"payer_id": client, "method": "card"}, "reservation_id": held["id"]
        })
    };
    assert_eq!(clinic.book(booking(&slot["parties"][0], &json!(clinic.ada))).await.0, StatusCode::OK);
    let (_, still) = clinic.search(json!({"from": at("09:00"), "to": at("10:00"),
        "parties": [{"client": {"tag": "guest"}, "service_id": clinic.float, "resources": [{"id": slot["parties"][1]["resources"][0]["id"]}]}]})).await;
    assert_eq!(still["slots"][0]["parties"][0]["waitlist"], json!(true), "Bo's place is still held");
    assert_eq!(clinic.book(booking(&slot["parties"][1], &bo["id"])).await.0, StatusCode::OK);
}
