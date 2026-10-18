//! Specification: docs/TPD-booking.md, section 10 (eligibility is the schedule).
//! Behaviour on a database with nothing booked. Seeded scenarios are in the other booking tests.

use chrono::{Duration, Utc};
use megh::booking::{Availability, Axes, BookableId, Calendar, Interval, ServiceId};
use sqlx::PgPool;
use uuid::Uuid;

fn day() -> Interval {
    let start = Utc::now();
    Interval { start, end: start + Duration::days(1) }
}

#[sqlx::test]
async fn a_combination_with_no_open_rule_has_no_open_time(pool: PgPool) {
    let axes = Axes { org: Uuid::new_v4(), service: ServiceId(Uuid::new_v4()), bookable: BookableId(Uuid::new_v4()) };
    assert_eq!(Calendar::new(pool).open(&axes, &day()).await.unwrap(), vec![]);
}
