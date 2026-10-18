//! Specification: docs/TPD-booking.md, sections 5 (principles), 7 (steps, needs, variants), 11 (search), 14 (scenarios a, b, h).

mod booking_support;

use booking_support::*;
use chrono::Utc;
use megh::booking::{Allocation, BookableRow, Change, Engagement, Party, Search, SearchRequest, ServiceRow, ServiceWant, Slot, Slots, Variant};
use sqlx::PgPool;
use uuid::Uuid;

struct World {
    pool: PgPool,
    org: Uuid,
}

async fn world(pool: PgPool) -> World {
    let org = org(&pool, "salon").await;
    World { pool, org }
}

impl World {
    async fn room(&self, capacity: u32) -> BookableRow {
        let room = bookable(&self.pool, self.org, "room", capacity, &[("kind", "room")]).await;
        open_rule(&self.pool, Where { bookable: Some(room.id), ..Where::default() }, 9, 8).await;
        room
    }

    async fn stylist(&self, tags: &[(&str, &str)]) -> BookableRow {
        let tags: Vec<_> = tags.iter().copied().chain([("role", "stylist")]).collect();
        let stylist = bookable(&self.pool, self.org, "stylist", 1, &tags).await;
        open_rule(&self.pool, Where { bookable: Some(stylist.id), ..Where::default() }, 9, 8).await;
        stylist
    }

    async fn haircut(&self, length: u64) -> ServiceRow {
        let service = service(&self.pool, "haircut", length, Some(is("role", "stylist")), is("kind", "room")).await;
        open_rule(&self.pool, Where { service: Some(service.id), ..Where::default() }, 9, 8).await;
        service
    }

    async fn find(&self, request: &SearchRequest) -> Vec<Slot> {
        Slots::new(self.pool.clone()).find(request).await.unwrap()
    }
}

fn starts(slot: &Slot) -> Vec<chrono::DateTime<Utc>> {
    slot.allocations.iter().map(|allocation| allocation.during.start).collect()
}

fn covered(allocations: &[&Allocation]) -> std::time::Duration {
    let start = allocations.iter().map(|a| a.during.start).min().unwrap();
    let end = allocations.iter().map(|a| a.during.end).max().unwrap();
    (end - start).to_std().unwrap()
}

#[sqlx::test]
async fn a_slot_engages_the_provider_and_the_room_for_the_step(pool: PgPool) {
    let w = world(pool).await;
    let (room, stylist, haircut) = (w.room(1).await, w.stylist(&[]).await, w.haircut(60).await);
    let slots = w.find(&request(haircut.id, whole_day(1))).await;
    assert!(!slots.is_empty());
    for slot in &slots {
        let (provider, room) = (allocation_of(slot, stylist.id), allocation_of(slot, room.id));
        assert_eq!((provider.len(), room.len()), (1, 1));
        assert_eq!(length(provider[0]), minutes(60));
        assert_eq!(provider[0].during, room[0].during);
        assert_eq!(provider[0].engagement, Engagement::Engaged);
        assert!(provider[0].during.start >= at(1, 9, 0) && provider[0].during.end <= at(1, 17, 0));
    }
}

#[sqlx::test]
async fn no_slot_overlaps_closed_time(pool: PgPool) {
    let w = world(pool).await;
    let (_room, stylist, haircut) = (w.room(1).await, w.stylist(&[]).await, w.haircut(60).await);
    blockout_rule(&w.pool, Where { bookable: Some(stylist.id), ..Where::default() }, daily(12), 60).await;
    let slots = w.find(&request(haircut.id, whole_day(1))).await;
    assert!(!slots.is_empty());
    assert!(slots.iter().flat_map(|slot| &slot.allocations).all(|a| !overlaps(&a.during, &between(1, 12, 13))));
}

#[sqlx::test]
async fn a_provider_without_an_open_rule_is_never_offered(pool: PgPool) {
    let w = world(pool).await;
    let room = w.room(1).await;
    let haircut = service(&w.pool, "haircut", 60, Some(is("role", "stylist")), is("kind", "room")).await;
    let eligible = w.stylist(&[]).await;
    let ineligible = bookable(&w.pool, w.org, "unscheduled", 1, &[("role", "stylist")]).await;
    let slots = w.find(&request(haircut.id, whole_day(1))).await;
    assert!(!slots.is_empty());
    assert!(slots.iter().all(|slot| allocation_of(slot, eligible.id).len() == 1 && allocation_of(slot, ineligible.id).is_empty()));
    assert!(slots.iter().all(|slot| allocation_of(slot, room.id).len() == 1));
}

#[sqlx::test]
async fn a_want_restricts_the_choice_to_matching_bookables(pool: PgPool) {
    let w = world(pool).await;
    let (_room, haircut) = (w.room(1).await, w.haircut(60).await);
    let (woman, man) = (w.stylist(&[("gender", "female")]).await, w.stylist(&[("gender", "male")]).await);
    let mut search = request(haircut.id, whole_day(1));
    search.parties[0].services[0].wants = vec![is("gender", "female")];
    let slots = w.find(&search).await;
    assert!(!slots.is_empty());
    assert!(slots.iter().all(|slot| allocation_of(slot, woman.id).len() == 1 && allocation_of(slot, man.id).is_empty()));
}

#[sqlx::test]
async fn a_want_nobody_meets_finds_nothing(pool: PgPool) {
    let w = world(pool).await;
    let (_room, _stylist, haircut) = (w.room(1).await, w.stylist(&[]).await, w.haircut(60).await);
    let mut search = request(haircut.id, whole_day(1));
    search.parties[0].services[0].wants = vec![is("view", "sea")];
    assert!(w.find(&search).await.is_empty());
}

#[sqlx::test]
async fn a_room_at_capacity_is_not_offered_while_taken(pool: PgPool) {
    let w = world(pool).await;
    let (room, _stylist, haircut) = (w.room(1).await, w.stylist(&[]).await, w.haircut(60).await);
    booked(&w.pool, room.id, between(1, 9, 11), Engagement::Engaged).await;
    let slots = w.find(&request(haircut.id, whole_day(1))).await;
    assert!(!slots.is_empty());
    assert!(slots.iter().flat_map(|slot| &slot.allocations).all(|a| !overlaps(&a.during, &between(1, 9, 11))));
}

#[sqlx::test]
async fn capacity_two_allows_a_second_booking_at_the_same_time(pool: PgPool) {
    let w = world(pool).await;
    let (room, _stylist, haircut) = (w.room(2).await, w.stylist(&[]).await, w.haircut(60).await);
    booked(&w.pool, room.id, between(1, 9, 11), Engagement::Engaged).await;
    let slots = w.find(&request(haircut.id, between(1, 9, 11))).await;
    assert!(!slots.is_empty());
}

#[sqlx::test]
async fn an_active_hold_blocks_and_an_expired_one_does_not(pool: PgPool) {
    let w = world(pool).await;
    let (room, _stylist, haircut) = (w.room(1).await, w.stylist(&[]).await, w.haircut(60).await);
    let hold = held(&w.pool, room.id, between(1, 9, 17), Utc::now() + chrono::Duration::minutes(10)).await;
    assert!(w.find(&request(haircut.id, whole_day(1))).await.is_empty());
    sqlx::query("UPDATE holds SET expires_at = now() - interval '1 minute' WHERE id = $1").bind(hold.id).execute(&w.pool).await.unwrap();
    assert!(!w.find(&request(haircut.id, whole_day(1))).await.is_empty());
}

#[sqlx::test]
async fn a_provider_who_is_free_can_serve_another_client(pool: PgPool) {
    let w = world(pool).await;
    let (colour_room, spare_room, stylist, haircut) = (w.room(1).await, w.room(1).await, w.stylist(&[]).await, w.haircut(30).await);
    booked(&w.pool, colour_room.id, between(1, 10, 11), Engagement::Engaged).await;
    booked(&w.pool, stylist.id, minutes_past(1, 10, 0, 10), Engagement::Engaged).await;
    booked(&w.pool, stylist.id, minutes_past(1, 10, 10, 40), Engagement::Free).await;
    booked(&w.pool, stylist.id, minutes_past(1, 10, 40, 60), Engagement::Engaged).await;
    let window = megh::booking::Interval { start: at(1, 10, 10), end: at(1, 10, 40) };
    let slots = w.find(&request(haircut.id, window)).await;
    assert_eq!(slots.len(), 1);
    assert_eq!(allocation_of(&slots[0], stylist.id)[0].during, window);
    assert_eq!(allocation_of(&slots[0], spare_room.id).len(), 1);
}

/// From `from` to `to` minutes past `hour`.
fn minutes_past(offset: i64, hour: u32, from: u32, to: u32) -> megh::booking::Interval {
    megh::booking::Interval { start: at(offset, hour, from), end: at(offset, hour, 0) + chrono::Duration::minutes(i64::from(to)) }
}

#[sqlx::test]
async fn a_variant_changes_duration_and_price_for_the_matching_provider(pool: PgPool) {
    let w = world(pool).await;
    let room = w.room(1).await;
    let (master, junior) = (w.stylist(&[("level", "master")]).await, w.stylist(&[("level", "junior")]).await);
    let step = step_of(60, Some(is("role", "stylist")), is("kind", "room"));
    let master_pays_more = Variant { when: is("level", "master"), duration: Some(Change::Add(minutes(15))), price_micros: Some(Change::Add(50_000_000)) };
    let colour = insert_service(&w.pool, "colour", vec![step], vec![master_pays_more]).await;
    open_rule(&w.pool, Where { service: Some(colour.id), ..Where::default() }, 9, 8).await;
    let slots = w.find(&request(colour.id, whole_day(1))).await;
    let master_slot = slots.iter().find(|slot| !allocation_of(slot, master.id).is_empty()).unwrap();
    let junior_slot = slots.iter().find(|slot| !allocation_of(slot, junior.id).is_empty()).unwrap();
    assert_eq!(length(allocation_of(master_slot, master.id)[0]), minutes(75));
    assert_eq!(master_slot.price.micros, 150_000_000);
    assert_eq!(length(allocation_of(junior_slot, junior.id)[0]), minutes(60));
    assert_eq!(junior_slot.price.micros, 100_000_000);
    assert!(!allocation_of(master_slot, room.id).is_empty());
}

fn step_of(length: u64, provider: Option<megh::booking::Condition>, room: megh::booking::Condition) -> megh::booking::Step {
    let needs = provider.into_iter().map(|who| need(who, Engagement::Engaged)).chain([need(room, Engagement::Engaged)]).collect();
    megh::booking::Step { duration: minutes(length), client: megh::booking::Presence::Present, needs, attributes: Default::default() }
}

fn timed(length: u64, client: megh::booking::Presence, needs: Vec<megh::booking::Need>) -> megh::booking::Step {
    megh::booking::Step { duration: minutes(length), client, needs, attributes: Default::default() }
}

#[sqlx::test]
async fn hair_colour_frees_the_stylist_between_prep_and_finish(pool: PgPool) {
    let w = world(pool).await;
    let (room, stylist) = (w.room(1).await, w.stylist(&[]).await);
    let who = || is("role", "stylist");
    let chair = || is("kind", "room");
    let present = megh::booking::Presence::Present;
    let steps = vec![
        timed(10, present, vec![need(who(), Engagement::Engaged), need(chair(), Engagement::Engaged)]),
        timed(30, present, vec![need(who(), Engagement::Free), need(chair(), Engagement::Engaged)]),
        timed(20, present, vec![need(who(), Engagement::Engaged), need(chair(), Engagement::Engaged)]),
    ];
    let colour = insert_service(&w.pool, "colour", steps, vec![]).await;
    open_rule(&w.pool, Where { service: Some(colour.id), ..Where::default() }, 9, 8).await;
    let slot = w.find(&request(colour.id, whole_day(1))).await.remove(0);
    let mut turns = allocation_of(&slot, stylist.id);
    turns.sort_by_key(|a| a.during.start);
    let shape: Vec<_> = turns.iter().map(|a| (length(a), a.engagement)).collect();
    assert_eq!(shape, vec![(minutes(10), Engagement::Engaged), (minutes(30), Engagement::Free), (minutes(20), Engagement::Engaged)]);
    assert!(turns.windows(2).all(|pair| pair[0].during.end == pair[1].during.start));
    assert_eq!(covered(&allocation_of(&slot, room.id)), minutes(60));
}

#[sqlx::test]
async fn cleanup_holds_the_room_after_the_client_and_provider_are_done(pool: PgPool) {
    let w = world(pool).await;
    let (room, stylist) = (w.room(1).await, w.stylist(&[]).await);
    let steps = vec![
        timed(50, megh::booking::Presence::Present, vec![need(is("role", "stylist"), Engagement::Engaged), need(is("kind", "room"), Engagement::Engaged)]),
        timed(10, megh::booking::Presence::Absent, vec![need(is("kind", "room"), Engagement::Engaged)]),
    ];
    let therapy = insert_service(&w.pool, "therapy", steps, vec![]).await;
    open_rule(&w.pool, Where { service: Some(therapy.id), ..Where::default() }, 9, 8).await;
    let slot = w.find(&request(therapy.id, whole_day(1))).await.remove(0);
    assert_eq!(covered(&allocation_of(&slot, stylist.id)), minutes(50));
    assert_eq!(covered(&allocation_of(&slot, room.id)), minutes(60));
}

#[sqlx::test]
async fn a_partys_services_follow_each_other_back_to_back(pool: PgPool) {
    let w = world(pool).await;
    let _room = w.room(1).await;
    let (cutter, colourist) = (w.stylist(&[("skill", "cut")]).await, w.stylist(&[("skill", "colour")]).await);
    let cut = service(&w.pool, "cut", 30, Some(is("skill", "cut")), is("kind", "room")).await;
    let colour = service(&w.pool, "colour", 60, Some(is("skill", "colour")), is("kind", "room")).await;
    for service in [&cut, &colour] {
        open_rule(&w.pool, Where { service: Some(service.id), ..Where::default() }, 9, 8).await;
    }
    let want = |service: &ServiceRow| ServiceWant { service: service.id, wants: vec![] };
    let mut search = request(cut.id, whole_day(1));
    search.parties = vec![Party { client: None, seats: 1, services: vec![want(&cut), want(&colour)] }];
    let slots = w.find(&search).await;
    assert!(!slots.is_empty());
    for slot in &slots {
        assert_eq!(allocation_of(slot, cutter.id)[0].during.end, allocation_of(slot, colourist.id)[0].during.start);
    }
}

#[sqlx::test]
async fn parties_share_one_room_at_the_same_time(pool: PgPool) {
    let w = world(pool).await;
    let suite = w.room(2).await;
    let (first, second) = (w.stylist(&[]).await, w.stylist(&[]).await);
    let massage = w.haircut(60).await;
    let party = || Party { client: None, seats: 1, services: vec![ServiceWant { service: massage.id, wants: vec![] }] };
    let mut search = request(massage.id, whole_day(1));
    search.parties = vec![party(), party()];
    let slots = w.find(&search).await;
    assert!(!slots.is_empty());
    for slot in &slots {
        assert_eq!(starts(slot).iter().min(), starts(slot).iter().max());
        assert_eq!(allocation_of(slot, first.id).len() + allocation_of(slot, second.id).len(), 2);
        assert_eq!(allocation_of(slot, suite.id).iter().map(|a| a.seats).sum::<u32>(), 2);
    }
}
