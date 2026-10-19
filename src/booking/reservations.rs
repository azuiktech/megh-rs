//! Holds, appointments and their status changes. A hold is an appointment with the status `held` until it is booked or
//! runs out. A slot is one appointment for each of its placements.

use chrono::{DateTime, Utc};
use sha2::{Digest, Sha256};
use sqlx::types::Json;
use sqlx::{PgConnection, PgPool, Postgres, Transaction};
use uuid::Uuid;

use super::busy::busy;
use super::rules::Rules;
use super::times::{clash, covers};
use super::{
    Allocation, Amount, Appointment, AppointmentId, AppointmentRow, Appointments, Attributes, Axes, BookableId, BookableRow, ClientId, Engagement, Error, IdempotencyKey, Interval, Payment,
    PaymentMethodId, PaymentMethodRow, Placement, Price, Reservations, Result, Role, SearchRequest, ServiceId, ServiceRow, Settlement, Slot, Status,
};
use crate::TableEntity;

type Tx<'a> = Transaction<'a, Postgres>;

/// The engaged rows of a room in a time with these statuses; a held row stops counting when it runs out.
const ACTIVE_ROWS: &str = "SELECT COUNT(*)::BIGINT FROM occupancies o JOIN appointments a ON a.id = o.appointment_id
    WHERE o.bookable_id = $1 AND o.engagement = 'engaged' AND o.status = ANY($5) AND o.deleted_at IS NULL AND o.starts_at < $3 AND o.ends_at > $2
    AND a.deleted_at IS NULL AND (a.status <> 'held' OR a.expires_at > now())
    AND ($4::uuid IS NULL OR o.appointment_id <> $4)";

const CAPACITY: [&str; 3] = ["held", "booked", "confirmed"];
const WAITLIST: [&str; 1] = ["waitlist"];

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

async fn rows_taken(connection: &mut PgConnection, bookable: BookableId, during: &Interval, ignoring: Option<Uuid>, statuses: &[&str]) -> Result<i64> {
    Ok(sqlx::query_scalar(ACTIVE_ROWS).bind(bookable).bind(during.start).bind(during.end).bind(ignoring).bind(statuses).fetch_one(connection).await?)
}

fn overlapping(left: &Allocation, right: &Allocation) -> bool {
    left.bookable == right.bookable && left.during.start < right.during.end && right.during.start < left.during.end
}

/// The client of the party of each placement; every party needs one.
fn clients_of(request: &SearchRequest, slot: &Slot) -> Result<Vec<ClientId>> {
    let missing = || Error::Invalid("every party needs a client".into());
    slot.placements.iter().map(|placement| request.parties.get(placement.party).and_then(|party| party.client).ok_or_else(missing)).collect()
}

/// A client named by the request must be free for the times held for them.
async fn check_clients(pool: &PgPool, request: &SearchRequest, slot: &Slot, ignoring: Option<Uuid>) -> Result<()> {
    let named: Vec<ClientId> = request.parties.iter().filter_map(|party| party.client).collect();
    let span = Interval { start: slot.placements.iter().map(|p| p.during.start).min().unwrap_or_default(), end: slot.placements.iter().map(|p| p.during.end).max().unwrap_or_default() };
    let calendars = busy(pool, &named, &span, ignoring).await?;
    let taken = slot.placements.iter().find_map(|placement| {
        let client = request.parties.get(placement.party)?.client?;
        calendars.get(&client).filter(|calendar| clash(calendar, &placement.during)).map(|_| client)
    });
    taken.map_or(Ok(()), |client| Err(Error::ClientBusy(client)))
}

/// The status each allocation of the slot gets: `booked` while its room has capacity, `waitlist` while it has a place.
async fn admit(tx: &mut Tx<'_>, rules: &Rules, request: &SearchRequest, slot: &Slot, bookables: &[BookableRow], ignoring: Option<Uuid>) -> Result<Vec<Status>> {
    let mut statuses: Vec<Status> = vec![];
    for (index, allocation) in slot.allocations.iter().enumerate() {
        if !engaged(allocation) {
            statuses.push(Status::Booked);
            continue;
        }
        let bookable = bookables.iter().find(|b| b.id == allocation.bookable).ok_or(Error::NotFound)?;
        if !is_open(rules, bookable, request, allocation)? {
            let service = request.parties.iter().flat_map(|party| &party.services).next().map(|want| want.service).ok_or(Error::NotFound)?;
            return Err(Error::NotOpen(Axes { org: bookable.org_id, service, bookable: bookable.id }));
        }
        let earlier = |status: Status| -> i64 { slot.allocations[..index].iter().zip(&statuses).filter(|(a, s)| engaged(a) && **s == status && overlapping(a, allocation)).count() as i64 };
        let used = rows_taken(tx.as_mut(), bookable.id, &allocation.during, ignoring, &CAPACITY).await? + earlier(Status::Booked);
        if used < i64::from(bookable.capacity) {
            statuses.push(Status::Booked);
            continue;
        }
        let waiting = rows_taken(tx.as_mut(), bookable.id, &allocation.during, ignoring, &WAITLIST).await? + earlier(Status::Waitlist);
        if waiting >= i64::from(bookable.waitlist) {
            return Err(Error::Occupied(bookable.id));
        }
        statuses.push(Status::Waitlist);
    }
    Ok(statuses)
}

/// A hold reserves capacity only: a place on a waitlist is taken by booking directly.
fn refuse_waitlist(slot: &Slot, statuses: &[Status]) -> Result<()> {
    match slot.allocations.iter().zip(statuses).find(|(_, status)| **status == Status::Waitlist) {
        Some((allocation, _)) => Err(Error::Occupied(allocation.bookable)),
        None => Ok(()),
    }
}

/// What makes two bookings the same request: the party, service and time of each placement, and the payments.
pub fn booking_print(placements: &[(usize, ServiceId, Interval)], payments: &[Payment]) -> String {
    fingerprint(&(placements, payments))
}

fn fingerprint(request: &impl serde::Serialize) -> String {
    let bytes = serde_json::to_vec(request).unwrap_or_default();
    hex::encode(Sha256::digest(bytes))
}

/// What the services look like to a payment method's `eligible` filter: `display_name`, `mode` and `attributes`.
fn subject(service: &ServiceRow) -> Attributes {
    serde_json::json!({"display_name": service.name, "mode": service.mode, "attributes": service.attributes})
}

/// The settlement after paying, or why not: the window of a pass is judged on the time of the appointment.
fn debit(method: PaymentMethodId, settlement: Settlement, amount: &Amount, appointment: Interval) -> Result<Settlement> {
    let outside = |valid: &Interval| appointment.start < valid.start || valid.end < appointment.end;
    match (settlement, amount) {
        (Settlement::OnSpot, _) => Ok(Settlement::OnSpot),
        (Settlement::Prepaid { valid: Some(valid), .. }, _) if outside(&valid) => Err(Error::Expired(method)),
        (Settlement::Prepaid { balance: Amount::Uses(have), valid }, Amount::Uses(used)) => {
            have.checked_sub(*used).map(|left| Settlement::Prepaid { balance: Amount::Uses(left), valid }).ok_or(Error::Insufficient(method))
        }
        (Settlement::Prepaid { balance: Amount::Money(have), valid }, Amount::Money(used)) if have.currency == used.currency => {
            let left = have.micros.checked_sub(used.micros).filter(|left| *left >= 0).ok_or(Error::Insufficient(method))?;
            Ok(Settlement::Prepaid { balance: Amount::Money(Price { micros: left, currency: have.currency }), valid })
        }
        _ => Err(Error::Insufficient(method)),
    }
}

async fn service_of(tx: &mut Tx<'_>, id: ServiceId) -> Result<ServiceRow> {
    sqlx::query_as("SELECT * FROM services WHERE id = $1").bind(id).fetch_optional(tx.as_mut()).await?.ok_or(Error::NotFound)
}

/// Debits the payment, if the method covers the service.
async fn pay(tx: &mut Tx<'_>, payment: &Payment, appointment: &AppointmentRow) -> Result<()> {
    let query = "SELECT * FROM payment_methods WHERE id = $1 AND deleted_at IS NULL FOR UPDATE";
    let method: PaymentMethodRow = sqlx::query_as(query).bind(payment.method).fetch_optional(tx.as_mut()).await?.ok_or(Error::NotFound)?;
    let service = service_of(tx, appointment.service_id).await?;
    if !method.eligible.holds(&subject(&service)) {
        return Err(Error::NotEligible(payment.method));
    }
    let settled = debit(payment.method, method.settlement.clone(), &payment.amount, Interval { start: appointment.starts_at, end: appointment.ends_at })?;
    sqlx::query("UPDATE payment_methods SET settlement = $2, updated_at = now() WHERE id = $1").bind(payment.method).bind(Json(settled)).execute(tx.as_mut()).await?;
    Ok(())
}

async fn locked(tx: &mut Tx<'_>, id: AppointmentId) -> Result<AppointmentRow> {
    sqlx::query_as("SELECT * FROM appointments WHERE id = $1 FOR UPDATE").bind(id).fetch_optional(tx.as_mut()).await?.ok_or(Error::NotFound)
}

async fn fetch(pool: &PgPool, id: AppointmentId) -> Result<AppointmentRow> {
    Ok(sqlx::query_as("SELECT * FROM appointments WHERE id = $1").bind(id).fetch_one(pool).await?)
}

/// The room or provider, or the client, whose time a row blocks.
enum Axis {
    Bookable(BookableId),
    Client(ClientId),
}

async fn insert_row(tx: &mut Tx<'_>, appointment: AppointmentId, axis: Axis, during: &Interval, engagement: Engagement, status: Status) -> Result<()> {
    let (bookable, client) = match axis {
        Axis::Bookable(bookable) => (Some(bookable), None),
        Axis::Client(client) => (None, Some(client)),
    };
    let query = "INSERT INTO occupancies (id, appointment_id, bookable_id, client_id, starts_at, ends_at, engagement, status, created_at, updated_at) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, now(), now())";
    sqlx::query(query).bind(Uuid::new_v4()).bind(appointment).bind(bookable).bind(client).bind(during.start).bind(during.end).bind(engagement).bind(status).execute(tx.as_mut()).await?;
    Ok(())
}

/// A proposed appointment: what one placement of a slot is for.
struct Proposal<'a> {
    placement: &'a Placement,
    client: ClientId,
    providers: Vec<BookableId>,
    room: BookableId,
    allocations: Vec<(&'a Allocation, Status)>,
}

/// The appointments a slot is: for each placement, its client, its room and its providers, with the status each of
/// its allocations was admitted with.
fn proposals<'a>(request: &SearchRequest, slot: &'a Slot, statuses: &[Status]) -> Result<Vec<Proposal<'a>>> {
    let clients = clients_of(request, slot)?;
    slot.placements
        .iter()
        .enumerate()
        .zip(clients)
        .map(|((index, placement), client)| {
            let allocations: Vec<_> = slot.allocations.iter().zip(statuses).filter(|(a, _)| a.placement == index).map(|(a, s)| (a, *s)).collect();
            let mut providers: Vec<BookableId> = allocations.iter().filter(|(a, _)| a.role == Role::Provider).map(|(a, _)| a.bookable).collect();
            providers.sort();
            providers.dedup();
            let room = allocations.iter().find(|(a, _)| a.role == Role::Room).map(|(a, _)| a.bookable).ok_or_else(|| Error::Invalid("the service needs a room".into()))?;
            Ok(Proposal { placement, client, providers, room, allocations })
        })
        .collect()
}

impl Proposal<'_> {
    /// `waitlist` when any room allocation had to wait.
    fn status(&self) -> Status {
        if self.allocations.iter().any(|(_, status)| *status == Status::Waitlist) { Status::Waitlist } else { Status::Booked }
    }
}

/// The appointment row and the rows of the time it blocks.
async fn insert_proposal(tx: &mut Tx<'_>, org: Uuid, proposal: &Proposal<'_>, status: Status, expires_at: Option<DateTime<Utc>>, payment: Option<Payment>) -> Result<AppointmentRow> {
    let data = Appointment {
        org_id: org,
        client_id: proposal.client,
        service_id: proposal.placement.service,
        provider_ids: proposal.providers.clone(),
        room_id: proposal.room,
        starts_at: proposal.placement.during.start,
        ends_at: proposal.placement.during.end,
        status,
        expires_at,
        payment,
        attributes: serde_json::json!({}),
    };
    let appointment = AppointmentRow::new(AppointmentId(Uuid::new_v4()), data);
    let query = "INSERT INTO appointments (id, org_id, client_id, service_id, provider_ids, room_id, starts_at, ends_at, status, expires_at, payment, attributes, created_at, updated_at) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, now(), now())";
    sqlx::query(query)
        .bind(appointment.id)
        .bind(appointment.org_id)
        .bind(appointment.client_id)
        .bind(appointment.service_id)
        .bind(&appointment.provider_ids)
        .bind(appointment.room_id)
        .bind(appointment.starts_at)
        .bind(appointment.ends_at)
        .bind(appointment.status)
        .bind(appointment.expires_at)
        .bind(appointment.payment.as_ref().map(Json))
        .bind(Json(&appointment.attributes))
        .execute(tx.as_mut())
        .await?;
    insert_rows(tx, appointment.id, proposal, status).await?;
    fetch_tx(tx, appointment.id).await
}

/// What an allocation is stored as: the status of the appointment, except that a room allocation that waits stays
/// `waitlist`.
fn stored(appointment: Status, admitted: Status) -> Status {
    if admitted == Status::Waitlist { Status::Waitlist } else { appointment }
}

async fn insert_rows(tx: &mut Tx<'_>, appointment: AppointmentId, proposal: &Proposal<'_>, status: Status) -> Result<()> {
    for (allocation, admitted) in &proposal.allocations {
        insert_row(tx, appointment, Axis::Bookable(allocation.bookable), &allocation.during, allocation.engagement, stored(status, *admitted)).await?;
    }
    insert_row(tx, appointment, Axis::Client(proposal.client), &proposal.placement.during, Engagement::Engaged, status).await
}

async fn remembered(tx: &mut Tx<'_>, key: &IdempotencyKey, print: &str) -> Result<Option<Vec<AppointmentId>>> {
    let seen: Option<(String, Vec<Uuid>)> = sqlx::query_as("SELECT fingerprint, appointment_ids FROM booking_requests WHERE request_id = $1").bind(&key.0).fetch_optional(tx.as_mut()).await?;
    match seen {
        None => Ok(None),
        Some((seen, ids)) if seen == print => Ok(Some(ids.into_iter().map(AppointmentId).collect())),
        Some(_) => Err(Error::KeyReused),
    }
}

async fn remember(tx: &mut Tx<'_>, key: &IdempotencyKey, print: &str, appointments: &[AppointmentRow]) -> Result<()> {
    let ids: Vec<Uuid> = appointments.iter().map(|appointment| appointment.id.0).collect();
    sqlx::query("INSERT INTO booking_requests (request_id, fingerprint, appointment_ids) VALUES ($1, $2, $3)").bind(&key.0).bind(print).bind(ids).execute(tx.as_mut()).await?;
    Ok(())
}

async fn fetch_all(pool: &PgPool, ids: &[AppointmentId]) -> Result<Vec<AppointmentRow>> {
    let raw: Vec<Uuid> = ids.iter().map(|id| id.0).collect();
    Ok(sqlx::query_as("SELECT * FROM appointments WHERE id = ANY($1) ORDER BY starts_at, id").bind(raw).fetch_all(pool).await?)
}

/// The status of the appointment and of all its rows.
async fn set_status(tx: &mut Tx<'_>, appointment: AppointmentId, status: Status) -> Result<()> {
    sqlx::query("UPDATE appointments SET status = $2, updated_at = now() WHERE id = $1").bind(appointment).bind(status).execute(tx.as_mut()).await?;
    sqlx::query("UPDATE occupancies SET status = $2, updated_at = now() WHERE appointment_id = $1 AND deleted_at IS NULL").bind(appointment).bind(status).execute(tx.as_mut()).await?;
    Ok(())
}

/// Waitlist to booked: every room it waits in must have the capacity now; the payment is debited now.
async fn promote(tx: &mut Tx<'_>, row: &AppointmentRow) -> Result<()> {
    let waiting: Vec<(BookableId, DateTime<Utc>, DateTime<Utc>)> =
        sqlx::query_as("SELECT bookable_id, starts_at, ends_at FROM occupancies WHERE appointment_id = $1 AND bookable_id IS NOT NULL AND status = 'waitlist' AND engagement = 'engaged' AND deleted_at IS NULL ORDER BY bookable_id")
            .bind(row.id)
            .fetch_all(tx.as_mut())
            .await?;
    for (bookable, start, end) in waiting {
        sqlx::query("SELECT id FROM bookables WHERE id = $1 FOR UPDATE").bind(bookable).execute(tx.as_mut()).await?;
        let room: BookableRow = sqlx::query_as("SELECT * FROM bookables WHERE id = $1").bind(bookable).fetch_one(tx.as_mut()).await?;
        if rows_taken(tx.as_mut(), bookable, &Interval { start, end }, Some(row.id.0), &CAPACITY).await? >= i64::from(room.capacity) {
            return Err(Error::Occupied(bookable));
        }
    }
    if let Some(payment) = &row.payment {
        pay(tx, payment, row).await?;
    }
    set_status(tx, row.id, Status::Booked).await
}

impl Reservations {
    /// The appointments an earlier booking with this key made, when it was the same request; `Error::KeyReused` when
    /// the key was used for another.
    pub async fn booked_under(&self, key: &IdempotencyKey, print: &str) -> Result<Option<Vec<AppointmentRow>>> {
        let mut tx = self.pool.begin().await?;
        match remembered(&mut tx, key, print).await? {
            Some(ids) => Ok(Some(fetch_all(&self.pool, &ids).await?)),
            None => Ok(None),
        }
    }

    /// Cancels the held appointments that have run out. A held appointment past `expires_at` is ignored by every
    /// query already; this frees its rows. Run it every minute or so.
    pub async fn expire_holds(&self) -> Result<usize> {
        let ids: Vec<AppointmentId> = sqlx::query_scalar("SELECT id FROM appointments WHERE status = 'held' AND expires_at <= now() AND deleted_at IS NULL").fetch_all(&self.pool).await?;
        for id in &ids {
            self.transition(*id, Status::Cancelled).await?;
        }
        Ok(ids.len())
    }
}

impl Appointments for Reservations {
    async fn hold(&self, request: &SearchRequest, slot: &Slot, until: DateTime<Utc>) -> Result<Vec<AppointmentRow>> {
        let rules = Rules::load(&self.pool).await?;
        let mut tx = self.pool.begin().await?;
        let bookables = lock_bookables(&mut tx, slot).await?;
        check_clients(&self.pool, request, slot, None).await?;
        let statuses = admit(&mut tx, &rules, request, slot, &bookables, None).await?;
        refuse_waitlist(slot, &statuses)?;
        let org = bookables.first().ok_or(Error::NotFound)?.org_id;
        let mut held = vec![];
        for proposal in proposals(request, slot, &statuses)? {
            held.push(insert_proposal(&mut tx, org, &proposal, Status::Held, Some(until), None).await?);
        }
        tx.commit().await?;
        Ok(held)
    }

    async fn book_held(&self, held: AppointmentId, payment: Payment, key: IdempotencyKey) -> Result<AppointmentRow> {
        let print = fingerprint(&(held, &payment));
        let mut tx = self.pool.begin().await?;
        if remembered(&mut tx, &key, &print).await?.is_some() {
            return fetch(&self.pool, held).await;
        }
        let row = locked(&mut tx, held).await?;
        if row.status != Status::Held {
            return Err(Error::Transition(row.status, Status::Booked));
        }
        if row.expires_at.is_none_or(|expires| expires <= Utc::now()) {
            return Err(Error::HoldExpired(held));
        }
        pay(&mut tx, &payment, &row).await?;
        sqlx::query("UPDATE appointments SET status = 'booked', expires_at = NULL, payment = $2, updated_at = now() WHERE id = $1").bind(held).bind(Json(&payment)).execute(tx.as_mut()).await?;
        sqlx::query("UPDATE occupancies SET status = 'booked', updated_at = now() WHERE appointment_id = $1 AND deleted_at IS NULL").bind(held).execute(tx.as_mut()).await?;
        let booked = fetch_tx(&mut tx, held).await?;
        remember(&mut tx, &key, &print, std::slice::from_ref(&booked)).await?;
        tx.commit().await?;
        Ok(booked)
    }

    async fn cancel(&self, appointment: AppointmentId) -> Result<()> {
        let live: Option<AppointmentRow> = sqlx::query_as("SELECT * FROM appointments WHERE id = $1 AND deleted_at IS NULL").bind(appointment).fetch_optional(&self.pool).await?;
        live.ok_or(Error::NotFound)?;
        self.transition(appointment, Status::Cancelled).await.map(|_| ())
    }

    async fn book(&self, request: &SearchRequest, slot: &Slot, payments: Vec<Payment>, key: IdempotencyKey) -> Result<Vec<AppointmentRow>> {
        let print = booking_print(&slot.placements.iter().map(|p| (p.party, p.service, p.during)).collect::<Vec<_>>(), &payments);
        if payments.len() != slot.placements.len() {
            return Err(Error::Invalid("a payment is needed for each placement of the slot".into()));
        }
        let rules = Rules::load(&self.pool).await?;
        let mut tx = self.pool.begin().await?;
        if let Some(earlier) = remembered(&mut tx, &key, &print).await? {
            return fetch_all(&self.pool, &earlier).await;
        }
        let bookables = lock_bookables(&mut tx, slot).await?;
        check_clients(&self.pool, request, slot, None).await?;
        let statuses = admit(&mut tx, &rules, request, slot, &bookables, None).await?;
        let org = bookables.first().ok_or(Error::NotFound)?.org_id;
        let mut booked = vec![];
        for (proposal, payment) in proposals(request, slot, &statuses)?.iter().zip(payments) {
            let status = proposal.status();
            let appointment = insert_proposal(&mut tx, org, proposal, status, None, Some(payment.clone())).await?;
            if status == Status::Booked {
                pay(&mut tx, &payment, &appointment).await?;
            }
            booked.push(appointment);
        }
        remember(&mut tx, &key, &print, &booked).await?;
        tx.commit().await?;
        Ok(booked)
    }

    async fn transition(&self, appointment: AppointmentId, to: Status) -> Result<AppointmentRow> {
        let mut tx = self.pool.begin().await?;
        let row = locked(&mut tx, appointment).await?;
        match (row.status, to) {
            (Status::Waitlist, Status::Booked) => promote(&mut tx, &row).await?,
            (Status::Held | Status::Waitlist | Status::Booked | Status::Confirmed, Status::Cancelled) | (Status::Booked, Status::Confirmed) => set_status(&mut tx, appointment, to).await?,
            (from, to) => return Err(Error::Transition(from, to)),
        }
        tx.commit().await?;
        if to == Status::Cancelled {
            row.soft_delete(&self.pool).await?;
        }
        fetch(&self.pool, appointment).await
    }

    async fn reschedule(&self, appointment: AppointmentId, request: &SearchRequest, slot: &Slot) -> Result<AppointmentRow> {
        let rules = Rules::load(&self.pool).await?;
        let mut tx = self.pool.begin().await?;
        let row = locked(&mut tx, appointment).await?;
        if row.deleted_at.is_some() || row.status == Status::Cancelled {
            return Err(Error::NotFound);
        }
        if row.status == Status::Held && row.expires_at.is_none_or(|expires| expires <= Utc::now()) {
            return Err(Error::HoldExpired(appointment));
        }
        let bookables = lock_bookables(&mut tx, slot).await?;
        check_clients(&self.pool, request, slot, Some(appointment.0)).await?;
        let statuses = admit(&mut tx, &rules, request, slot, &bookables, Some(appointment.0)).await?;
        let proposals = proposals(request, slot, &statuses)?;
        let [proposal] = proposals.as_slice() else { return Err(Error::Invalid("the new slot is for one appointment".into())) };
        if proposal.client != row.client_id || proposal.placement.service != row.service_id {
            return Err(Error::Invalid("the new slot is for another client or service than the appointment".into()));
        }
        if row.status != Status::Waitlist && proposal.status() == Status::Waitlist {
            return Err(Error::Occupied(proposal.room));
        }
        sqlx::query("UPDATE occupancies SET deleted_at = now(), updated_at = now() WHERE appointment_id = $1 AND deleted_at IS NULL").bind(appointment).execute(tx.as_mut()).await?;
        let query = "UPDATE appointments SET provider_ids = $2, room_id = $3, starts_at = $4, ends_at = $5, updated_at = now() WHERE id = $1";
        sqlx::query(query).bind(appointment).bind(&proposal.providers).bind(proposal.room).bind(proposal.placement.during.start).bind(proposal.placement.during.end).execute(tx.as_mut()).await?;
        insert_rows(&mut tx, appointment, proposal, row.status).await?;
        tx.commit().await?;
        fetch(&self.pool, appointment).await
    }
}

async fn fetch_tx(tx: &mut Tx<'_>, id: AppointmentId) -> Result<AppointmentRow> {
    Ok(sqlx::query_as("SELECT * FROM appointments WHERE id = $1").bind(id).fetch_one(tx.as_mut()).await?)
}
