//! Specification: docs/TPD-booking.md, section 7 (waitlist entry): clients wait for a target in the order they asked.

mod booking_support;

use booking_support::*;
use chrono::{Duration, Utc};
use megh::booking::{ClientRow, OccurrenceId, Queue, Target, Waitlist, WaitlistEntry};
use uuid::Uuid;
use sqlx::PgPool;

#[sqlx::test]
async fn the_waitlist_keeps_request_order_per_target(pool: PgPool) {
    let first = client(&pool).await;
    let haircut = service(&pool, "haircut", 60, None, is("kind", "room")).await;
    let (target, elsewhere) = (Target::Window(haircut.id, whole_day(1)), Target::Window(haircut.id, whole_day(2)));
    let queue = Queue::new(pool.clone());
    let later = client(&pool).await;
    let entry = |client: &ClientRow, target: &Target, minutes_ago: i64| WaitlistEntry {
        org_id: None,
        client_id: client.id,
        target: target.clone(),
        requested_at: Utc::now() - Duration::minutes(minutes_ago),
        attributes: Default::default(),
    };
    queue.join(entry(&later, &target, 1)).await.unwrap();
    queue.join(entry(&first, &target, 5)).await.unwrap();
    queue.join(entry(&first, &elsewhere, 9)).await.unwrap();
    let waiting: Vec<_> = queue.waiting(&target).await.unwrap().into_iter().map(|row| row.client_id).collect();
    assert_eq!(waiting, vec![first.id, later.id]);
}

#[sqlx::test]
async fn an_occurrence_has_its_own_queue(pool: PgPool) {
    let (talk, other) = (Target::Occurrence(OccurrenceId(Uuid::new_v4())), Target::Occurrence(OccurrenceId(Uuid::new_v4())));
    let guest = client(&pool).await;
    let queue = Queue::new(pool.clone());
    let entry = |target: &Target| WaitlistEntry { org_id: None, client_id: guest.id, target: target.clone(), requested_at: Utc::now(), attributes: Default::default() };
    let joined = queue.join(entry(&talk)).await.unwrap();
    assert_eq!(joined.client_id, guest.id);
    assert_eq!(queue.waiting(&talk).await.unwrap().len(), 1);
    assert!(queue.waiting(&other).await.unwrap().is_empty());
}
