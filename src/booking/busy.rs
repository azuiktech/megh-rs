//! A client's own calendar: the times they are already booked, waitlisted or held.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use super::times::{insert, Times};
use super::{ClientId, Interval, Result};

const BUSY: &str = "SELECT o.client_id, o.starts_at, o.ends_at FROM occupancies o JOIN appointments a ON a.id = o.appointment_id
    WHERE o.client_id = ANY($1) AND o.deleted_at IS NULL AND o.status <> 'cancelled' AND o.starts_at < $3 AND o.ends_at > $2
    AND a.deleted_at IS NULL AND (a.status <> 'held' OR a.expires_at > now())
    AND ($4::uuid IS NULL OR o.appointment_id <> $4)";

/// When each of the clients is busy within the window: by a live appointment of any status but cancelled, a hold that has not expired included, found by one range scan on the clients' own rows.
pub(super) async fn busy(pool: &PgPool, clients: &[ClientId], within: &Interval, ignoring: Option<Uuid>) -> Result<HashMap<ClientId, Times>> {
    let rows: Vec<(ClientId, DateTime<Utc>, DateTime<Utc>)> = sqlx::query_as(BUSY).bind(clients).bind(within.start).bind(within.end).bind(ignoring).fetch_all(pool).await?;
    Ok(rows.into_iter().fold(HashMap::new(), |mut calendars, (client, start, end)| {
        insert(calendars.entry(client).or_default(), &Interval { start, end });
        calendars
    }))
}
