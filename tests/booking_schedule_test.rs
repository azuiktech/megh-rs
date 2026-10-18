//! Specification: docs/TPD-booking.md, section 10 (availability, exceptions and inheritance).

mod booking_support;

use booking_support::*;
use chrono::Weekday;
use megh::booking::{Axes, Calendar, Grouped, Interval, Availability};
use sqlx::PgPool;
use uuid::Uuid;

struct World {
    pool: PgPool,
    org: Uuid,
    room: megh::booking::BookableRow,
    service: megh::booking::ServiceRow,
}

async fn world(pool: PgPool) -> World {
    let org = org(&pool, "clinic").await;
    let room = bookable(&pool, org, "room", 1, &[("kind", "room")]).await;
    let service = service(&pool, "cleaning", 60, None, is("kind", "room")).await;
    World { pool, org, room, service }
}

impl World {
    fn axes(&self) -> Axes {
        Axes { org: self.org, service: self.service.id, bookable: self.room.id }
    }

    async fn open(&self, within: &Interval) -> Vec<Interval> {
        Calendar::new(self.pool.clone()).open(&self.axes(), within).await.unwrap()
    }

    fn here(&self) -> Where {
        Where { service: Some(self.service.id), bookable: Some(self.room.id), ..Where::default() }
    }
}

#[sqlx::test]
async fn an_open_rule_gives_its_hours(pool: PgPool) {
    let w = world(pool).await;
    open_rule(&w.pool, w.here(), 9, 8).await;
    assert_eq!(w.open(&whole_day(1)).await, vec![between(1, 9, 17)]);
}

#[sqlx::test]
async fn open_time_is_clipped_to_the_window(pool: PgPool) {
    let w = world(pool).await;
    open_rule(&w.pool, w.here(), 9, 8).await;
    assert_eq!(w.open(&between(1, 10, 12)).await, vec![between(1, 10, 12)]);
}

#[sqlx::test]
async fn a_combination_with_no_matching_open_rule_has_no_time(pool: PgPool) {
    let w = world(pool).await;
    let other = service(&w.pool, "other", 30, None, is("kind", "room")).await;
    open_rule(&w.pool, Where { service: Some(other.id), bookable: Some(w.room.id), ..Where::default() }, 9, 8).await;
    assert_eq!(w.open(&whole_day(1)).await, vec![]);
}

#[sqlx::test]
async fn a_rule_naming_another_bookable_does_not_open_this_one(pool: PgPool) {
    let w = world(pool).await;
    let other = bookable(&w.pool, w.org, "other", 1, &[("kind", "room")]).await;
    open_rule(&w.pool, Where { service: Some(w.service.id), bookable: Some(other.id), ..Where::default() }, 9, 8).await;
    assert_eq!(w.open(&whole_day(1)).await, vec![]);
}

#[sqlx::test]
async fn an_omitted_axis_means_any(pool: PgPool) {
    let w = world(pool).await;
    let second = bookable(&w.pool, w.org, "second", 1, &[("kind", "room")]).await;
    open_rule(&w.pool, Where { service: Some(w.service.id), ..Where::default() }, 9, 8).await;
    let axes = Axes { bookable: second.id, ..w.axes() };
    let calendar = Calendar::new(w.pool.clone());
    assert_eq!(w.open(&whole_day(1)).await, vec![between(1, 9, 17)]);
    assert_eq!(calendar.open(&axes, &whole_day(1)).await.unwrap(), vec![between(1, 9, 17)]);
}

#[sqlx::test]
async fn specific_weekdays_open_only_on_those_days(pool: PgPool) {
    let w = world(pool).await;
    rule(&w.pool, megh::booking::Kind::Open, w.here(), weekly(&[Weekday::Wed, Weekday::Thu], 15), minutes(180)).await;
    for offset in 0..7 {
        let expected = match weekday(offset) {
            Weekday::Wed | Weekday::Thu => vec![between(offset, 15, 18)],
            _ => vec![],
        };
        assert_eq!(w.open(&whole_day(offset)).await, expected, "day {offset}");
    }
}

#[sqlx::test]
async fn a_recurring_exception_is_subtracted_from_open_time(pool: PgPool) {
    let w = world(pool).await;
    open_rule(&w.pool, w.here(), 9, 8).await;
    blockout_rule(&w.pool, Where { bookable: Some(w.room.id), ..Where::default() }, daily(12), 60).await;
    assert_eq!(w.open(&whole_day(1)).await, vec![between(1, 9, 12), between(1, 13, 17)]);
}

#[sqlx::test]
async fn a_holiday_closes_only_its_day(pool: PgPool) {
    let w = world(pool).await;
    open_rule(&w.pool, w.here(), 9, 8).await;
    blockout_rule(&w.pool, Where { org: Some(w.org), ..Where::default() }, once(2, 0), 24 * 60).await;
    assert_eq!(w.open(&whole_day(1)).await, vec![between(1, 9, 17)]);
    assert_eq!(w.open(&whole_day(2)).await, vec![]);
    assert_eq!(w.open(&whole_day(3)).await, vec![between(3, 9, 17)]);
}

#[sqlx::test]
async fn blocked_hours_on_a_room_close_that_room_only(pool: PgPool) {
    let w = world(pool).await;
    let other = bookable(&w.pool, w.org, "other", 1, &[("kind", "room")]).await;
    open_rule(&w.pool, Where { org: Some(w.org), ..Where::default() }, 9, 8).await;
    blockout_rule(&w.pool, Where { bookable: Some(w.room.id), ..Where::default() }, daily(10), 60).await;
    let calendar = Calendar::new(w.pool.clone());
    let other_axes = Axes { bookable: other.id, ..w.axes() };
    assert_eq!(w.open(&whole_day(1)).await, vec![between(1, 9, 10), between(1, 11, 17)]);
    assert_eq!(calendar.open(&other_axes, &whole_day(1)).await.unwrap(), vec![between(1, 9, 17)]);
}

#[sqlx::test]
async fn a_deleted_rule_is_ignored(pool: PgPool) {
    let w = world(pool).await;
    open_rule(&w.pool, w.here(), 9, 8).await;
    sqlx::query("UPDATE schedules SET deleted_at = now()").execute(&w.pool).await.unwrap();
    assert_eq!(w.open(&whole_day(1)).await, vec![]);
}

struct Chain {
    brand: Uuid,
    franchise: Uuid,
    location: Uuid,
}

/// The location is in the franchise, and the franchise is in the brand.
async fn chain(w: &World) -> Chain {
    let (brand, franchise, location) = (org(&w.pool, "brand").await, org(&w.pool, "franchise").await, org(&w.pool, "location").await);
    group(&w.pool, Grouped::Org, franchise, brand).await;
    group(&w.pool, Grouped::Org, location, franchise).await;
    Chain { brand, franchise, location }
}

fn at_org(org: Uuid) -> Where {
    Where { org: Some(org), ..Where::default() }
}

#[sqlx::test]
async fn a_rule_on_a_group_applies_to_its_members_transitively(pool: PgPool) {
    let w = world(pool).await;
    let c = chain(&w).await;
    open_rule(&w.pool, at_org(c.brand), 8, 12).await;
    let axes = Axes { org: c.location, ..w.axes() };
    assert_eq!(Calendar::new(w.pool.clone()).open(&axes, &whole_day(1)).await.unwrap(), vec![between(1, 8, 20)]);
}

#[sqlx::test]
async fn the_nearest_open_hours_win_and_are_not_merged(pool: PgPool) {
    let w = world(pool).await;
    let c = chain(&w).await;
    open_rule(&w.pool, at_org(c.brand), 8, 12).await;
    open_rule(&w.pool, at_org(c.location), 10, 4).await;
    let axes = Axes { org: c.location, ..w.axes() };
    assert_eq!(Calendar::new(w.pool.clone()).open(&axes, &whole_day(1)).await.unwrap(), vec![between(1, 10, 14)]);
}

#[sqlx::test]
async fn a_holiday_at_the_brand_closes_its_locations(pool: PgPool) {
    let w = world(pool).await;
    let c = chain(&w).await;
    open_rule(&w.pool, at_org(c.location), 9, 8).await;
    blockout_rule(&w.pool, at_org(c.brand), once(1, 0), 24 * 60).await;
    let calendar = Calendar::new(w.pool.clone());
    let axes = Axes { org: c.location, ..w.axes() };
    assert_eq!(calendar.open(&axes, &whole_day(1)).await.unwrap(), vec![]);
    assert_eq!(calendar.open(&axes, &whole_day(2)).await.unwrap(), vec![between(2, 9, 17)]);
}

#[sqlx::test]
async fn with_two_parents_the_longest_path_decides_which_is_nearest(pool: PgPool) {
    let w = world(pool).await;
    let c = chain(&w).await;
    group(&w.pool, Grouped::Org, c.location, c.brand).await;
    open_rule(&w.pool, at_org(c.brand), 8, 12).await;
    open_rule(&w.pool, at_org(c.franchise), 10, 4).await;
    let axes = Axes { org: c.location, ..w.axes() };
    assert_eq!(Calendar::new(w.pool.clone()).open(&axes, &whole_day(1)).await.unwrap(), vec![between(1, 10, 14)]);
}
