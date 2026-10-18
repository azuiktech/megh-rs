//! Holds, appointments and cancellations.

use chrono::{DateTime, Utc};
use sha2::{Digest, Sha256};
use sqlx::types::Json;
use sqlx::{PgConnection, Postgres, Transaction};
use uuid::Uuid;

use super::rules::Rules;
use super::times::covers;
use super::{
    Allocation, Amount, Appointment, AppointmentId, AppointmentRow, Appointments, Attributes, Axes, BookableId, BookableRow, BookedLine, ClientId,
    Confirmation, Engagement, Error, HoldId, HoldRow, IdempotencyKey, Interval, OrgId, Party, Payment, PaymentMethodRow, Price, Reservations, Result,
    SearchRequest, Settlement, Slot,
};
use crate::TableEntity;

type Tx<'a> = Transaction<'a, Postgres>;

const ACTIVE_SEATS: &str = "SELECT COALESCE(SUM(o.seats), 0)::BIGINT FROM occupancies o LEFT JOIN holds h ON h.id = o.hold_id
    WHERE o.bookable_id = $1 AND o.engagement = 'engaged' AND o.deleted_at IS NULL AND o.starts_at < $3 AND o.ends_at > $2
    AND (o.appointment_id IS NOT NULL OR (h.deleted_at IS NULL AND h.expires_at > now()))";

fn invalid(error: serde_json::Error) -> Error {
    Error::Invalid(error.to_string())
}

/// A copy through the serialized form: parties hold conditions, which are trait objects without `Clone`.
fn copy_parties(parties: &[Party]) -> Result<Vec<Party>> {
    serde_json::from_value(serde_json::to_value(parties).map_err(invalid)?).map_err(invalid)
}

fn engaged(allocation: &Allocation) -> bool {
    allocation.engagement == Engagement::Engaged
}

async fn lock_bookables(tx: &mut Tx<'_>, slot: &Slot) -> Result<Vec<BookableRow>> {
    let ids: Vec<Uuid> = slot.allocations.iter().map(|a| a.bookable.0).collect();
    Ok(sqlx::query_as("SELECT * FROM bookables WHERE id = ANY($1) AND deleted_at IS NULL ORDER BY id FOR UPDATE").bind(ids).fetch_all(tx.as_mut()).await?)
}

/// Open for any service the request asks for.
fn is_open(rules: &Rules, bookable: &BookableRow, request: &SearchRequest, allocation: &Allocation) -> Result<bool> {
    let services = request.parties.iter().flat_map(|party| party.services.iter().map(|want| want.service));
    let opened = services.map(|service| {
        let axes = Axes { org: bookable.org_id, service, bookable: bookable.id };
        Ok(covers(&rules.times(&axes, &allocation.during)?, &allocation.during))
    });
    Ok(opened.collect::<Result<Vec<_>>>()?.into_iter().any(|open| open))
}

async fn seats_taken(connection: &mut PgConnection, bookable: BookableId, during: &Interval) -> Result<i64> {
    Ok(sqlx::query_scalar(ACTIVE_SEATS).bind(bookable).bind(during.start).bind(during.end).fetch_one(connection).await?)
}

fn overlapping(left: &Allocation, right: &Allocation) -> bool {
    left.bookable == right.bookable && left.during.start < right.during.end && right.during.start < left.during.end
}

async fn admit(tx: &mut Tx<'_>, rules: &Rules, request: &SearchRequest, slot: &Slot, bookables: &[BookableRow]) -> Result<()> {
    for (index, allocation) in slot.allocations.iter().enumerate().filter(|(_, a)| engaged(a)) {
        let bookable = bookables.iter().find(|b| b.id == allocation.bookable).ok_or(Error::NotFound)?;
        if !is_open(rules, bookable, request, allocation)? {
            let service = request.parties.iter().flat_map(|party| &party.services).next().map(|want| want.service).ok_or(Error::NotFound)?;
            return Err(Error::NotOpen(Axes { org: bookable.org_id, service, bookable: bookable.id }));
        }
        let earlier: i64 = slot.allocations[..index].iter().filter(|a| engaged(a) && overlapping(a, allocation)).map(|a| i64::from(a.seats)).sum();
        if seats_taken(tx.as_mut(), bookable.id, &allocation.during).await? + earlier + i64::from(allocation.seats) > i64::from(bookable.capacity) {
            return Err(Error::Occupied(bookable.id));
        }
    }
    Ok(())
}

async fn insert_occupancy(tx: &mut Tx<'_>, owner: (Option<HoldId>, Option<AppointmentId>), allocation: &Allocation) -> Result<()> {
    let query = "INSERT INTO occupancies (id, hold_id, appointment_id, bookable_id, starts_at, ends_at, engagement, seats, created_at, updated_at) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, now(), now())";
    sqlx::query(query)
        .bind(Uuid::new_v4())
        .bind(owner.0)
        .bind(owner.1)
        .bind(allocation.bookable)
        .bind(allocation.during.start)
        .bind(allocation.during.end)
        .bind(allocation.engagement)
        .bind(i32::try_from(allocation.seats).unwrap_or(i32::MAX))
        .execute(tx.as_mut())
        .await?;
    Ok(())
}

async fn zone(tx: &mut Tx<'_>, org: Option<OrgId>) -> Result<String> {
    let found: Option<Option<String>> = match org {
        Some(org) => sqlx::query_scalar("SELECT timezone FROM organizations WHERE id = $1").bind(org).fetch_optional(tx.as_mut()).await?,
        None => None,
    };
    Ok(found.flatten().unwrap_or_else(|| "UTC".into()))
}

async fn insert_hold(tx: &mut Tx<'_>, request: &SearchRequest, slot: &Slot, until: DateTime<Utc>, org: Option<OrgId>) -> Result<HoldId> {
    let id = HoldId(Uuid::new_v4());
    let tz = zone(tx, org).await?;
    let query = "INSERT INTO holds (id, org_id, tz, parties, placements, expires_at, attributes, created_at, updated_at) VALUES ($1, $2, $3, $4, $5, $6, $7, now(), now())";
    let parties = Json(copy_parties(&request.parties)?);
    sqlx::query(query).bind(id).bind(org).bind(tz).bind(parties).bind(Json(&slot.placements)).bind(until).bind(Json(Attributes::new())).execute(tx.as_mut()).await?;
    for allocation in &slot.allocations {
        insert_occupancy(tx, (Some(id), None), allocation).await?;
    }
    Ok(id)
}

async fn appointment_by_key(tx: &mut Tx<'_>, key: &IdempotencyKey) -> Result<Option<AppointmentRow>> {
    Ok(sqlx::query_as("SELECT * FROM appointments WHERE idempotency_key = $1").bind(key).fetch_optional(tx.as_mut()).await?)
}

async fn live_hold(tx: &mut Tx<'_>, id: HoldId) -> Result<HoldRow> {
    let hold: HoldRow = sqlx::query_as("SELECT * FROM holds WHERE id = $1 AND deleted_at IS NULL FOR UPDATE").bind(id).fetch_optional(tx.as_mut()).await?.ok_or(Error::NotFound)?;
    match hold.expires_at > Utc::now() {
        true => Ok(hold),
        false => Err(Error::HoldExpired(id)),
    }
}

fn fingerprint(hold: HoldId, confirmation: &Confirmation) -> String {
    let bytes = serde_json::to_vec(&(hold, confirmation)).unwrap_or_default();
    hex::encode(Sha256::digest(bytes))
}

/// One line for each service of each party, with the time and price of its placement.
fn booked_lines(hold: &HoldRow, clients: &[ClientId]) -> Result<Vec<BookedLine>> {
    hold.placements
        .iter()
        .map(|placement| {
            let party = hold.parties.get(placement.party).ok_or(Error::NotFound)?;
            let client = clients.get(placement.party).copied().or(party.client).ok_or(Error::NotFound)?;
            let duration = (placement.during.end - placement.during.start).to_std().map_err(|error| Error::Invalid(error.to_string()))?;
            Ok(BookedLine { client, service: placement.service, seats: party.seats, duration, price: placement.price.clone() })
        })
        .collect()
}

fn debit(settlement: Settlement, amount: &Amount, now: DateTime<Utc>) -> Option<Settlement> {
    match (settlement, amount) {
        (Settlement::OnSpot, _) => Some(Settlement::OnSpot),
        (Settlement::Prepaid { valid: Some(valid), .. }, _) if now < valid.start || valid.end < now => None,
        (Settlement::Prepaid { balance: Amount::Uses(have), valid }, Amount::Uses(used)) => have.checked_sub(*used).map(|left| Settlement::Prepaid { balance: Amount::Uses(left), valid }),
        (Settlement::Prepaid { balance: Amount::Money(have), valid }, Amount::Money(used)) if have.currency == used.currency => {
            let left = Price { micros: have.micros.checked_sub(used.micros).filter(|left| *left >= 0)?, currency: have.currency };
            Some(Settlement::Prepaid { balance: Amount::Money(left), valid })
        }
        _ => None,
    }
}

async fn pay(tx: &mut Tx<'_>, payment: &Payment) -> Result<()> {
    let query = "SELECT * FROM payment_methods WHERE id = $1 AND deleted_at IS NULL FOR UPDATE";
    let method: PaymentMethodRow = sqlx::query_as(query).bind(payment.method).fetch_optional(tx.as_mut()).await?.ok_or(Error::NotFound)?;
    let settled = debit(method.settlement.clone(), &payment.amount, Utc::now()).ok_or(Error::Insufficient(payment.method))?;
    sqlx::query("UPDATE payment_methods SET settlement = $2, updated_at = now() WHERE id = $1").bind(payment.method).bind(Json(settled)).execute(tx.as_mut()).await?;
    Ok(())
}

async fn insert_appointment(tx: &mut Tx<'_>, appointment: &AppointmentRow) -> Result<()> {
    let query = "INSERT INTO appointments (id, org_id, tz, lines, payer_id, payments, idempotency_key, fingerprint, attributes, created_at, updated_at) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, now(), now())";
    sqlx::query(query)
        .bind(appointment.id)
        .bind(appointment.org_id)
        .bind(&appointment.tz)
        .bind(Json(&appointment.lines))
        .bind(appointment.payer_id)
        .bind(Json(&appointment.payments))
        .bind(&appointment.idempotency_key)
        .bind(&appointment.fingerprint)
        .bind(Json(&appointment.attributes))
        .execute(tx.as_mut())
        .await?;
    Ok(())
}

/// The hold's occupancies become the appointment's, then the hold is deleted.
async fn hand_over(tx: &mut Tx<'_>, hold: HoldId, appointment: AppointmentId) -> Result<()> {
    sqlx::query("UPDATE occupancies SET hold_id = NULL, appointment_id = $2, updated_at = now() WHERE hold_id = $1").bind(hold).bind(appointment).execute(tx.as_mut()).await?;
    sqlx::query("UPDATE holds SET deleted_at = now(), updated_at = now() WHERE id = $1").bind(hold).execute(tx.as_mut()).await?;
    Ok(())
}

impl Appointments for Reservations {
    async fn hold(&self, request: &SearchRequest, slot: &Slot, until: DateTime<Utc>) -> Result<HoldRow> {
        let rules = Rules::load(&self.pool).await?;
        let mut tx = self.pool.begin().await?;
        let bookables = lock_bookables(&mut tx, slot).await?;
        admit(&mut tx, &rules, request, slot, &bookables).await?;
        let id = insert_hold(&mut tx, request, slot, until, bookables.first().map(|b| b.org_id)).await?;
        let row: HoldRow = sqlx::query_as("SELECT * FROM holds WHERE id = $1").bind(id).fetch_one(tx.as_mut()).await?;
        tx.commit().await?;
        Ok(row)
    }

    async fn confirm(&self, hold: HoldId, confirmation: Confirmation, key: IdempotencyKey) -> Result<AppointmentRow> {
        let print = fingerprint(hold, &confirmation);
        let mut tx = self.pool.begin().await?;
        if let Some(earlier) = appointment_by_key(&mut tx, &key).await? {
            return if earlier.fingerprint == print { Ok(earlier) } else { Err(Error::KeyReused) };
        }
        let held = live_hold(&mut tx, hold).await?;
        let lines = booked_lines(&held, &confirmation.clients)?;
        for payment in &confirmation.payments {
            pay(&mut tx, payment).await?;
        }
        let data = Appointment {
            org_id: held.org_id,
            tz: held.tz.clone(),
            lines,
            payer_id: confirmation.payer,
            payments: confirmation.payments,
            idempotency_key: key,
            fingerprint: print,
            attributes: confirmation.attributes,
        };
        let appointment = AppointmentRow::new(AppointmentId(hold.0), data);
        insert_appointment(&mut tx, &appointment).await?;
        hand_over(&mut tx, hold, appointment.id).await?;
        tx.commit().await?;
        Ok(appointment)
    }

    async fn cancel(&self, appointment: AppointmentId) -> Result<()> {
        let row: Option<AppointmentRow> = sqlx::query_as("SELECT * FROM appointments WHERE id = $1 AND deleted_at IS NULL").bind(appointment).fetch_optional(&self.pool).await?;
        let deleted = row.ok_or(Error::NotFound)?.soft_delete(&self.pool).await?;
        deleted.map(|_| ()).ok_or(Error::NotFound)
    }
}
