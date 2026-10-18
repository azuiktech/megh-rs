//! Specification: docs/TPD-booking.md, R4 (parties, `party_offset`, `service_gap`, `next_day`) and the org scope of a search.

mod booking_support;

use booking_support::*;
use megh::booking::{BookableRow, Engagement, Grouped, Interval, Party, Search, SearchRequest, ServiceRow, ServiceWant, Slot, Slots};
use sqlx::PgPool;
use uuid::Uuid;

fn party(service: &ServiceRow) -> Party {
    Party { client: None, seats: 1, services: vec![ServiceWant { service: service.id, wants: vec![] }] }
}

async fn stylist(pool: &PgPool, org: Uuid) -> BookableRow {
    let stylist = bookable(pool, org, "stylist", 1, &[("role", "stylist")]).await;
    open_rule(pool, Where { bookable: Some(stylist.id), ..Where::default() }, 9, 8).await;
    stylist
}

async fn find(pool: &PgPool, request: &SearchRequest) -> Vec<Slot> {
    Slots::new(pool.clone()).find(request).await.unwrap()
}

fn start_gap(slot: &Slot) -> chrono::Duration {
    let starts: Vec<_> = slot.placements.iter().map(|placement| placement.during.start).collect();
    *starts.iter().max().unwrap() - *starts.iter().min().unwrap()
}

#[sqlx::test]
async fn parties_start_together_unless_an_offset_is_allowed(pool: PgPool) {
    let org = org(&pool, "salon").await;
    let room = bookable(&pool, org, "suite", 2, &[("kind", "room")]).await;
    open_rule(&pool, Where { bookable: Some(room.id), ..Where::default() }, 9, 8).await;
    let (_first, _second) = (stylist(&pool, org).await, stylist(&pool, org).await);
    let massage = service(&pool, "massage", 60, Some(is("role", "stylist")), is("kind", "room")).await;
    let mut search = request(massage.id, between(1, 9, 11));
    search.parties = vec![party(&massage), party(&massage)];
    let together = find(&pool, &search).await;
    assert!(!together.is_empty());
    assert!(together.iter().all(|slot| start_gap(slot).is_zero()));
    search.party_offset = minutes(30);
    let offset = find(&pool, &search).await;
    assert!(offset.iter().any(|slot| start_gap(slot) == chrono::Duration::minutes(30)));
    assert!(offset.iter().all(|slot| start_gap(slot) <= chrono::Duration::minutes(30)));
}

/// The cutter works 9:00 to 9:30 only and the colourist is busy until 10:00, so a cut and a colour fit only with a gap.
async fn cut_then_colour(pool: &PgPool) -> SearchRequest {
    let org = org(pool, "salon").await;
    let room = bookable(pool, org, "room", 1, &[("kind", "room")]).await;
    open_rule(pool, Where { bookable: Some(room.id), ..Where::default() }, 9, 8).await;
    let cutter = bookable(pool, org, "cutter", 1, &[("skill", "cut")]).await;
    rule(pool, megh::booking::Kind::Open, Where { bookable: Some(cutter.id), ..Where::default() }, daily(9), minutes(30)).await;
    let colourist = stylist_with(pool, org, "colour").await;
    booked(pool, colourist.id, between(1, 9, 10), Engagement::Engaged).await;
    let cut = service(pool, "cut", 30, Some(is("skill", "cut")), is("kind", "room")).await;
    let colour = service(pool, "colour", 30, Some(is("skill", "colour")), is("kind", "room")).await;
    for service in [&cut, &colour] {
        open_rule(pool, Where { service: Some(service.id), ..Where::default() }, 9, 8).await;
    }
    let mut search = request(cut.id, Interval { start: at(1, 9, 0), end: at(1, 10, 30) });
    search.parties = vec![Party { client: None, seats: 1, services: vec![ServiceWant { service: cut.id, wants: vec![] }, ServiceWant { service: colour.id, wants: vec![] }] }];
    search
}

async fn stylist_with(pool: &PgPool, org: Uuid, skill: &str) -> BookableRow {
    let colourist = bookable(pool, org, "colourist", 1, &[("skill", skill)]).await;
    open_rule(pool, Where { bookable: Some(colourist.id), ..Where::default() }, 9, 8).await;
    colourist
}

#[sqlx::test]
async fn services_run_back_to_back_unless_a_gap_is_allowed(pool: PgPool) {
    let mut search = cut_then_colour(&pool).await;
    assert!(find(&pool, &search).await.is_empty());
    search.service_gap = minutes(30);
    let slots = find(&pool, &search).await;
    assert!(!slots.is_empty());
    for slot in &slots {
        let (cut, colour) = (&slot.placements[0], &slot.placements[1]);
        assert_eq!(colour.during.start - cut.during.end, chrono::Duration::minutes(30));
    }
}

#[sqlx::test]
async fn placements_say_when_each_service_of_each_party_takes_place(pool: PgPool) {
    let org = org(&pool, "salon").await;
    let room = bookable(&pool, org, "suite", 2, &[("kind", "room")]).await;
    open_rule(&pool, Where { bookable: Some(room.id), ..Where::default() }, 9, 8).await;
    let (_first, _second) = (stylist(&pool, org).await, stylist(&pool, org).await);
    let massage = service(&pool, "massage", 60, Some(is("role", "stylist")), is("kind", "room")).await;
    let mut search = request(massage.id, between(1, 9, 17));
    search.parties = vec![party(&massage), party(&massage)];
    let slot = find(&pool, &search).await.remove(0);
    let parties: Vec<_> = slot.placements.iter().map(|placement| placement.party).collect();
    assert_eq!(parties, vec![0, 1]);
    assert!(slot.placements.iter().all(|placement| placement.service == massage.id && placement.during == between(1, 9, 10)));
}

#[sqlx::test]
async fn a_limit_caps_the_slots(pool: PgPool) {
    let org = org(&pool, "salon").await;
    let (_room, _stylist) = (bookable(&pool, org, "room", 1, &[("kind", "room")]).await, stylist(&pool, org).await);
    let haircut = service(&pool, "haircut", 30, Some(is("role", "stylist")), is("kind", "room")).await;
    open_rule(&pool, Where { service: Some(haircut.id), ..Where::default() }, 9, 8).await;
    let mut search = request(haircut.id, whole_day(1));
    search.limit = Some(2);
    assert_eq!(find(&pool, &search).await.len(), 2);
}

#[sqlx::test]
async fn a_search_for_an_org_sees_its_own_bookables_and_those_grouped_under_it(pool: PgPool) {
    let (brand, location, other) = (org(&pool, "brand").await, org(&pool, "location").await, org(&pool, "other").await);
    group(&pool, Grouped::Org, location, brand).await;
    let inside = stylist(&pool, location).await;
    let outside = stylist(&pool, other).await;
    for org in [location, other] {
        let room = bookable(&pool, org, "room", 1, &[("kind", "room")]).await;
        open_rule(&pool, Where { bookable: Some(room.id), ..Where::default() }, 9, 8).await;
    }
    let haircut = service(&pool, "haircut", 30, Some(is("role", "stylist")), is("kind", "room")).await;
    let mut search = request(haircut.id, whole_day(1));
    search.org = Some(brand);
    let slots = find(&pool, &search).await;
    assert!(!slots.is_empty());
    assert!(slots.iter().all(|slot| allocation_of(slot, inside.id).len() == 1 && allocation_of(slot, outside.id).is_empty()));
}

#[sqlx::test]
async fn next_day_is_the_first_day_with_a_slot_after_the_window(pool: PgPool) {
    let org = org(&pool, "salon").await;
    let (room, stylist) = (bookable(&pool, org, "room", 1, &[("kind", "room")]).await, bookable(&pool, org, "stylist", 1, &[("role", "stylist")]).await);
    let haircut = service(&pool, "haircut", 30, Some(is("role", "stylist")), is("kind", "room")).await;
    for bookable in [room.id, stylist.id] {
        rule(&pool, megh::booking::Kind::Open, Where { bookable: Some(bookable), ..Where::default() }, once(3, 9), minutes(480)).await;
    }
    let search = request(haircut.id, whole_day(1));
    let slots = Slots::new(pool.clone());
    assert!(slots.find(&search).await.unwrap().is_empty());
    assert_eq!(slots.next_day(&search, 10).await.unwrap(), Some(day(3).date_naive()));
    assert_eq!(slots.next_day(&search, 1).await.unwrap(), None);
}
