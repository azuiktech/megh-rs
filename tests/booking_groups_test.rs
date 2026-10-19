//! Specification: docs/TPD-booking.md, section 10 (a grouping cycle is rejected) and R3 (local time across daylight saving).

mod booking_support;

use booking_support::*;
use chrono::{TimeZone, Utc};
use megh::booking::{Availability, Axes, Calendar, Error, Grouped, Interval, Kind, Recurrence};
use sqlx::PgPool;

#[sqlx::test]
async fn a_grouping_cycle_is_an_error(pool: PgPool) {
    let (first, second) = (org(&pool, "first").await, org(&pool, "second").await);
    group(&pool, Grouped::Org, first, second).await;
    group(&pool, Grouped::Org, second, first).await;
    let room = bookable(&pool, first, "room", 1, &[]).await;
    let service = service(&pool, first, "service", 30, None, is("kind", "room")).await;
    let axes = Axes { org: first, service: service.id, bookable: room.id };
    let result = Calendar::new(pool).open(&axes, &whole_day(1)).await;
    assert!(matches!(result, Err(Error::Cycle)));
}

#[sqlx::test]
async fn local_hours_stay_local_when_daylight_saving_ends(pool: PgPool) {
    let org = org(&pool, "clinic").await;
    let room = bookable(&pool, org, "room", 1, &[]).await;
    let service = service(&pool, org, "service", 30, None, is("kind", "room")).await;
    let hours: Recurrence = "DTSTART;TZID=America/New_York:20261030T090000\nRRULE:FREQ=DAILY".parse().unwrap();
    rule(&pool, Kind::Open, Where { service: Some(service.id), bookable: Some(room.id), ..Where::default() }, hours, minutes(480)).await;
    let axes = Axes { org, service: service.id, bookable: room.id };
    let day = |month, date| {
        let start = Utc.with_ymd_and_hms(2026, month, date, 0, 0, 0).unwrap();
        Interval { start, end: start + chrono::Duration::days(1) }
    };
    let nine_to_five = |month, date, start_hour| Interval { start: Utc.with_ymd_and_hms(2026, month, date, start_hour, 0, 0).unwrap(), end: Utc.with_ymd_and_hms(2026, month, date, start_hour + 8, 0, 0).unwrap() };
    let calendar = Calendar::new(pool);
    assert_eq!(calendar.open(&axes, &day(10, 31)).await.unwrap(), vec![nine_to_five(10, 31, 13)]);
    assert_eq!(calendar.open(&axes, &day(11, 2)).await.unwrap(), vec![nine_to_five(11, 2, 14)]);
}
