//! Specification: docs/TPD-booking.md, section 7 (waitlist entry): clients wait for a target in the order they asked.

mod booking_support;

use booking_support::*;
use chrono::{Duration, Utc};
use megh::booking::{ClientRow, Queue, Target, Waitlist, WaitlistEntry};
use sqlx::PgPool;

#[sqlx::test]
async fn the_waitlist_keeps_request_order_per_target(pool: PgPool) {
    let org = org(&pool, "salon").await;
    let first = client_of(&pool, org).await;
    let haircut = service(&pool, org, "haircut", 60, None, is("kind", "room")).await;
    let (target, elsewhere) = (Target::Window(haircut.id, whole_day(1)), Target::Window(haircut.id, whole_day(2)));
    let queue = Queue::new(pool.clone());
    let later = client_of(&pool, org).await;
    let entry = |client: &ClientRow, target: &Target, minutes_ago: i64| WaitlistEntry {
        org_id: org,
        client_id: client.id,
        target: target.clone(),
        requested_at: Utc::now() - Duration::minutes(minutes_ago),
        attributes: serde_json::json!({}),
    };
    queue.join(entry(&later, &target, 1)).await.unwrap();
    queue.join(entry(&first, &target, 5)).await.unwrap();
    queue.join(entry(&first, &elsewhere, 9)).await.unwrap();
    let waiting: Vec<_> = queue.waiting(&target).await.unwrap().into_iter().map(|row| row.client_id).collect();
    assert_eq!(waiting, vec![first.id, later.id]);
}
