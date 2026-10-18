//! How an appointment, booked or held, looks over REST, and how they are found.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use serde_json::{json, Value};
use sqlx::PgPool;
use uuid::Uuid;

use super::common::{money, seconds};
use super::error::ApiError;
use crate::booking::{AppointmentRow, HoldRow, OccupancyRow, Payment};

fn zone_of(name: &str) -> Tz {
    name.parse().unwrap_or(Tz::UTC)
}

fn local(time: DateTime<Utc>, zone: Tz) -> DateTime<Tz> {
    time.with_timezone(&zone)
}

fn allocations(occupancies: &[&OccupancyRow], zone: Tz) -> Vec<Value> {
    occupancies
        .iter()
        .map(|o| json!({"bookable_id": o.bookable_id, "start_time": local(o.starts_at, zone), "end_time": local(o.ends_at, zone), "engagement": o.engagement, "seats": o.seats}))
        .collect()
}

fn span(occupancies: &[&OccupancyRow], zone: Tz) -> (Option<DateTime<Tz>>, Option<DateTime<Tz>>) {
    let start = occupancies.iter().map(|o| o.starts_at).min().map(|time| local(time, zone));
    let end = occupancies.iter().map(|o| o.ends_at).max().map(|time| local(time, zone));
    (start, end)
}

/// The ids a filter can look for with `:`, such as `client_ids:"..."`.
fn ids<T: serde::Serialize>(ids: impl Iterator<Item = T>) -> Vec<Value> {
    let mut found: Vec<Value> = ids.filter_map(|id| serde_json::to_value(id).ok()).collect();
    found.sort_by_key(|id| id.to_string());
    found.dedup();
    found
}

fn payments(payments: &[Payment]) -> Vec<Value> {
    payments.iter().map(|payment| json!({"method": payment.method, "amount": match &payment.amount {
        crate::booking::Amount::Uses(uses) => json!({"uses": uses}),
        crate::booking::Amount::Money(price) => json!({"money": money(price)}),
    }})).collect()
}

/// The appointment as a resource: `state` is `booked`, or `cancelled` once deleted.
pub(super) fn appointment(row: &AppointmentRow, occupancies: &[&OccupancyRow], org: Uuid) -> Value {
    let zone = zone_of(&row.tz);
    let (start, end) = span(occupancies, zone);
    let lines: Vec<Value> = row.lines.iter().map(|l| json!({"client_id": l.client, "service_id": l.service, "seats": l.seats, "duration": seconds(l.duration), "price": money(&l.price)})).collect();
    json!({
        "name": format!("org/{org}/appointments/{}", row.id.0),
        "id": row.id,
        "state": if row.deleted_at.is_some() { "cancelled" } else { "booked" },
        "start_time": start,
        "end_time": end,
        "lines": lines,
        "client_ids": ids(row.lines.iter().map(|l| l.client)),
        "service_ids": ids(row.lines.iter().map(|l| l.service)),
        "bookable_ids": ids(occupancies.iter().map(|o| o.bookable_id)),
        "allocations": allocations(occupancies, zone),
        "payer_id": row.payer_id,
        "payments": payments(&row.payments),
        "attributes": row.attributes,
        "create_time": local(row.created_at, zone),
        "update_time": local(row.updated_at, zone),
        "delete_time": row.deleted_at.map(|time| local(time, zone)),
    })
}

/// The hold as a resource: `state` is `held` until it is confirmed (then it is the appointment) or deleted.
pub(super) fn hold(row: &HoldRow, occupancies: &[&OccupancyRow], org: Uuid) -> Value {
    let zone = zone_of(&row.tz);
    let (start, end) = span(occupancies, zone);
    let lines: Vec<Value> = row
        .placements
        .iter()
        .map(|p| {
            let client = row.parties.get(p.party).and_then(|party| party.client);
            json!({"client_id": client, "service_id": p.service, "duration": seconds((p.during.end - p.during.start).to_std().unwrap_or_default()), "price": money(&p.price)})
        })
        .collect();
    json!({
        "name": format!("org/{org}/appointments/{}", row.id.0),
        "id": row.id,
        "state": if row.deleted_at.is_some() { "cancelled" } else { "held" },
        "start_time": start,
        "end_time": end,
        "expire_time": local(row.expires_at, zone),
        "lines": lines,
        "client_ids": ids(row.parties.iter().filter_map(|party| party.client)),
        "service_ids": ids(row.placements.iter().map(|p| p.service)),
        "bookable_ids": ids(occupancies.iter().map(|o| o.bookable_id)),
        "allocations": allocations(occupancies, zone),
        "attributes": row.attributes,
        "create_time": local(row.created_at, zone),
        "update_time": local(row.updated_at, zone),
        "delete_time": row.deleted_at.map(|time| local(time, zone)),
    })
}

/// The occupancies of appointments and holds, by owner. Deleted ones are included: a cancelled appointment still shows its time.
pub(super) async fn occupancies(pool: &PgPool, owners: &[Uuid]) -> Result<HashMap<Uuid, Vec<OccupancyRow>>, ApiError> {
    let rows: Vec<OccupancyRow> = sqlx::query_as("SELECT * FROM occupancies WHERE appointment_id = ANY($1) OR hold_id = ANY($1) ORDER BY starts_at, id").bind(owners).fetch_all(pool).await?;
    Ok(rows.into_iter().fold(HashMap::new(), |mut by_owner, row| {
        if let Some(owner) = row.appointment_id.map(|id| id.0).or(row.hold_id.map(|id| id.0)) {
            by_owner.entry(owner).or_default().push(row);
        }
        by_owner
    }))
}

/// Every appointment and unexpired hold of the orgs, as resources, earliest first.
pub(super) async fn all(pool: &PgPool, scope: &[Uuid], org: Uuid, show_deleted: bool) -> Result<Vec<Value>, ApiError> {
    let shown = if show_deleted { "" } else { " AND deleted_at IS NULL" };
    let appointments: Vec<AppointmentRow> = sqlx::query_as(&format!("SELECT * FROM appointments WHERE org_id = ANY($1){shown}")).bind(scope).fetch_all(pool).await?;
    let holds: Vec<HoldRow> = sqlx::query_as("SELECT * FROM holds WHERE org_id = ANY($1) AND deleted_at IS NULL AND expires_at > now()").bind(scope).fetch_all(pool).await?;
    let owners: Vec<Uuid> = appointments.iter().map(|row| row.id.0).chain(holds.iter().map(|row| row.id.0)).collect();
    let held = occupancies(pool, &owners).await?;
    let of = |id: Uuid| held.get(&id).map(|rows| rows.iter().collect::<Vec<_>>()).unwrap_or_default();
    let mut items: Vec<Value> = appointments.iter().map(|row| appointment(row, &of(row.id.0), org)).chain(holds.iter().map(|row| hold(row, &of(row.id.0), org))).collect();
    items.sort_by_key(|item| (item["start_time"].as_str().map(str::to_owned), item["id"].as_str().map(str::to_owned)));
    Ok(items)
}
