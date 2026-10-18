//! A client's own calendar: the times they are already booked or held.

use std::collections::HashMap;

use sqlx::PgPool;

use super::times::{insert, Times};
use super::{AppointmentRow, ClientId, HoldRow, Result};

fn add(calendars: &mut HashMap<ClientId, Times>, client: ClientId, during: &super::Interval) {
    insert(calendars.entry(client).or_default(), during);
}

/// When each of the clients is booked, by a live appointment, or held, by a hold that has not expired.
pub(super) async fn busy(pool: &PgPool, clients: &[ClientId]) -> Result<HashMap<ClientId, Times>> {
    if clients.is_empty() {
        return Ok(HashMap::new());
    }
    let names: Vec<String> = clients.iter().map(|client| client.0.to_string()).collect();
    let booked: Vec<AppointmentRow> = sqlx::query_as("SELECT * FROM appointments a WHERE a.deleted_at IS NULL AND EXISTS (SELECT 1 FROM jsonb_array_elements(a.lines) l WHERE l->>'client' = ANY($1))").bind(&names).fetch_all(pool).await?;
    let held: Vec<HoldRow> = sqlx::query_as("SELECT * FROM holds h WHERE h.deleted_at IS NULL AND h.expires_at > now() AND EXISTS (SELECT 1 FROM jsonb_array_elements(h.parties) p WHERE p->>'client' = ANY($1))").bind(&names).fetch_all(pool).await?;
    let mut calendars = HashMap::new();
    booked.iter().flat_map(|row| row.lines.iter()).filter(|line| clients.contains(&line.client)).for_each(|line| add(&mut calendars, line.client, &line.during));
    for hold in &held {
        hold.placements.iter().filter_map(|placement| Some((hold.parties.get(placement.party)?.client?, placement))).filter(|(client, _)| clients.contains(client)).for_each(|(client, placement)| add(&mut calendars, client, &placement.during));
    }
    Ok(calendars)
}
