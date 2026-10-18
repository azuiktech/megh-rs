use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::{
    Attributes, BookableId, AppointmentId, ClientId, Condition, Engagement, HoldId, Interval, OrgId,
    OccupancyId, OccurrenceId, PaymentMethodId, Price, Result, ServiceId, WaitlistEntryId,
};
use crate::{Entity, Table};

/// One service wanted by a party. Every `wants` condition must hold for at least one bookable chosen for it.
#[derive(Debug, Serialize, Deserialize)]
pub struct ServiceWant {
    pub service: ServiceId,
    pub wants: Vec<Condition>,
}

/// A client with the services they want, in order.
#[derive(Debug, Serialize, Deserialize)]
pub struct Party {
    pub client: Option<ClientId>,
    pub seats: u32,
    pub services: Vec<ServiceWant>,
}

/// Parties start together and a party's services run back to back, unless the offsets allow more:
/// `party_offset` is the largest difference between the parties' start times, `service_gap` the largest
/// gap between one party's consecutive services. Starts and gaps are tried every `slot_interval`.
#[derive(Debug, Serialize, Deserialize)]
pub struct SearchRequest {
    /// Bookables of this org and of the orgs grouped under it; every org when empty.
    pub org: Option<OrgId>,
    pub parties: Vec<Party>,
    pub within: Interval,
    pub party_offset: Duration,
    pub service_gap: Duration,
    pub slot_interval: Duration,
    pub limit: Option<usize>,
}

/// The only thing that blocks time.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Allocation {
    pub bookable: BookableId,
    pub during: Interval,
    pub engagement: Engagement,
    pub seats: u32,
}

/// When one party's service takes place.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Placement {
    pub party: usize,
    pub service: ServiceId,
    pub during: Interval,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Slot {
    pub allocations: Vec<Allocation>,
    pub placements: Vec<Placement>,
    pub price: Price,
}

/// A semi-formed booking: a temporary block that may have no client or payment yet.
#[derive(Debug, Serialize, Deserialize, Table)]
#[cfg_attr(feature = "postgres", derive(sqlx::FromRow))]
#[table(name = "holds")]
pub struct Hold {
    pub org_id: Option<OrgId>,
    pub tz: String,
    #[cfg_attr(feature = "postgres", sqlx(json))]
    pub parties: Vec<Party>,
    pub expires_at: DateTime<Utc>,
    #[cfg_attr(feature = "postgres", sqlx(json))]
    pub attributes: Attributes,
}

/// An allocation as stored: the row that blocks time, owned by a hold or by a booking, never both.
/// Expired holds and soft-deleted appointments no longer block.
#[derive(Debug, Serialize, Deserialize, Table)]
#[cfg_attr(feature = "postgres", derive(sqlx::FromRow))]
#[table(name = "occupancies")]
pub struct Occupancy {
    pub hold_id: Option<HoldId>,
    pub appointment_id: Option<AppointmentId>,
    pub bookable_id: BookableId,
    pub starts_at: DateTime<Utc>,
    pub ends_at: DateTime<Utc>,
    pub engagement: Engagement,
    #[cfg_attr(feature = "postgres", sqlx(try_from = "i32"))]
    pub seats: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BookedLine {
    pub client: ClientId,
    pub service: ServiceId,
    pub seats: u32,
    pub duration: Duration,
    pub price: Price,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Amount {
    Money(Price),
    Uses(u32),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Payment {
    pub method: PaymentMethodId,
    pub amount: Amount,
}

/// Booked, or soft-deleted when cancelled. Late cancel, attendance and notes are attributes.
#[derive(Debug, Serialize, Deserialize, Table)]
#[cfg_attr(feature = "postgres", derive(sqlx::FromRow))]
#[table(name = "appointments")]
pub struct Appointment {
    pub org_id: Option<OrgId>,
    pub tz: String,
    #[cfg_attr(feature = "postgres", sqlx(json))]
    pub lines: Vec<BookedLine>,
    pub payer_id: ClientId,
    #[cfg_attr(feature = "postgres", sqlx(json))]
    pub payments: Vec<Payment>,
    pub idempotency_key: IdempotencyKey,
    /// Of the confirmation; the same key with another fingerprint is `Error::KeyReused`.
    pub fingerprint: String,
    #[cfg_attr(feature = "postgres", sqlx(json))]
    pub attributes: Attributes,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Confirmation {
    pub clients: Vec<ClientId>,
    pub payer: ClientId,
    pub payments: Vec<Payment>,
    pub attributes: Attributes,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "postgres", derive(sqlx::Type), sqlx(transparent))]
pub struct IdempotencyKey(pub String);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Settlement {
    Prepaid { balance: Amount, valid: Option<Interval> },
    OnSpot,
}

/// Pass, voucher, coupon, insurance, cash, card. `owner` is empty for shared methods.
#[derive(Debug, Serialize, Deserialize, Table)]
#[cfg_attr(feature = "postgres", derive(sqlx::FromRow))]
#[table(name = "payment_methods")]
pub struct PaymentMethod {
    pub org_id: Option<OrgId>,
    pub name: String,
    pub owner_id: Option<ClientId>,
    #[cfg_attr(feature = "postgres", sqlx(json))]
    pub eligible: Condition,
    #[cfg_attr(feature = "postgres", sqlx(json))]
    pub settlement: Settlement,
    #[cfg_attr(feature = "postgres", sqlx(json))]
    pub attributes: Attributes,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Target {
    Occurrence(OccurrenceId),
    Window(ServiceId, Interval),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Table)]
#[cfg_attr(feature = "postgres", derive(sqlx::FromRow))]
#[table(name = "waitlist_entries")]
pub struct WaitlistEntry {
    pub client_id: ClientId,
    #[cfg_attr(feature = "postgres", sqlx(json))]
    pub target: Target,
    pub requested_at: DateTime<Utc>,
    #[cfg_attr(feature = "postgres", sqlx(json))]
    pub attributes: Attributes,
}

/// Persisted forms. Soft delete, `created_at` and `updated_at` come from `Entity`.
pub type OccupancyRow = Entity<OccupancyId, Occupancy>;
pub type HoldRow = Entity<HoldId, Hold>;
pub type AppointmentRow = Entity<AppointmentId, Appointment>;
pub type PaymentMethodRow = Entity<PaymentMethodId, PaymentMethod>;
pub type WaitlistEntryRow = Entity<WaitlistEntryId, WaitlistEntry>;

/// Slots that satisfy every need of every party with no conflict.
#[allow(async_fn_in_trait)]
pub trait Search {
    async fn find(&self, request: &SearchRequest) -> Result<Vec<Slot>>;

    /// The first day, in the org's time zone, with any slot after the window, looking at most `horizon_days` ahead.
    async fn next_day(&self, request: &SearchRequest, horizon_days: u32) -> Result<Option<chrono::NaiveDate>>;
}

#[allow(async_fn_in_trait)]
pub trait Appointments {
    async fn hold(&self, request: &SearchRequest, slot: &Slot, until: DateTime<Utc>) -> Result<HoldRow>;
    async fn confirm(&self, hold: HoldId, confirmation: Confirmation, key: IdempotencyKey) -> Result<AppointmentRow>;
    async fn cancel(&self, booking: AppointmentId) -> Result<()>;
}

#[allow(async_fn_in_trait)]
pub trait Waitlist {
    async fn join(&self, entry: WaitlistEntry) -> Result<WaitlistEntryRow>;
    async fn waiting(&self, target: &Target) -> Result<Vec<WaitlistEntryRow>>;
}
