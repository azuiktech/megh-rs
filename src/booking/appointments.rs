use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::{
    Attributes, BookableId, AppointmentId, ClientId, Condition, Engagement, Interval, OrgId,
    OccupancyId, PaymentMethodId, Price, Result, Role, ServiceId,
};
use super::model::{not_blank, within_orgs};
use crate::{Entity, Table};

/// One service wanted by a party. Every `wants` condition must hold for at least one bookable chosen for it.
#[derive(Debug, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ServiceWant {
    #[schema(value_type = String, format = Uuid)]
    pub service: ServiceId,
    #[schema(value_type = Vec<Object>)]
    #[serde(default)]
    pub wants: Vec<Condition>,
}

/// A client with the services they want, in order.
#[derive(Debug, Serialize, Deserialize, utoipa::ToSchema)]
pub struct Party {
    #[schema(value_type = Option<String>, format = Uuid)]
    pub client: Option<ClientId>,
    pub services: Vec<ServiceWant>,
}

/// Parties start together and a party's services run back to back, unless the offsets allow more:
/// `party_offset` is the largest difference between the parties' start times, `service_gap` the largest
/// gap between one party's consecutive services. Starts and gaps are tried every `slot_interval`.
#[derive(Debug, Serialize, Deserialize, utoipa::ToSchema)]
pub struct SearchRequest {
    /// Bookables of this org and of the orgs grouped under it; every org when empty.
    #[schema(value_type = Option<String>, format = Uuid)]
    pub org: Option<OrgId>,
    pub parties: Vec<Party>,
    pub within: Interval,
    /// ISO 8601, such as `PT15M`; none when absent.
    #[serde(default, with = "jiff::fmt::serde::unsigned_duration::required")]
    #[schema(value_type = String, format = Duration)]
    pub party_offset: Duration,
    /// ISO 8601; none when absent.
    #[serde(default, with = "jiff::fmt::serde::unsigned_duration::required")]
    #[schema(value_type = String, format = Duration)]
    pub service_gap: Duration,
    /// ISO 8601; 15 minutes when absent.
    #[serde(default = "quarter_hour", with = "jiff::fmt::serde::unsigned_duration::required")]
    #[schema(value_type = String, format = Duration)]
    pub slot_interval: Duration,
    pub limit: Option<usize>,
}

fn quarter_hour() -> Duration {
    Duration::from_secs(15 * 60)
}

/// Where an appointment stands. `booked` and `confirmed` use the capacity of a room, `waitlist` a place of its
/// waitlist; `cancelled` uses nothing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "postgres", derive(sqlx::Type), sqlx(type_name = "text", rename_all = "snake_case"))]
pub enum Status {
    /// A slot kept for a while, until `expires_at`; it uses capacity like a booked appointment.
    Held,
    Waitlist,
    #[default]
    Booked,
    Confirmed,
    Cancelled,
}

/// The only thing that blocks time.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct Allocation {
    #[schema(value_type = String, format = Uuid)]
    pub bookable: BookableId,
    pub during: Interval,
    pub engagement: Engagement,
    /// Whether the bookable is the room or a provider of the service.
    pub role: Role,
    /// The placement of the slot this belongs to.
    pub placement: usize,
    /// `waitlist` when the room is over capacity but within its waitlist.
    #[serde(default)]
    pub status: Status,
}

/// When one party's service takes place for the client: from the first step the client is present to the last.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct Placement {
    pub party: usize,
    #[schema(value_type = String, format = Uuid)]
    pub service: ServiceId,
    pub during: Interval,
    pub price: Price,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct Slot {
    pub allocations: Vec<Allocation>,
    pub placements: Vec<Placement>,
    pub price: Price,
}

/// An allocation as stored: the row that blocks time, owned by an appointment.
/// Expired holds and soft-deleted appointments no longer block.
#[derive(Debug, Serialize, Deserialize, Table)]
#[cfg_attr(feature = "postgres", derive(sqlx::FromRow))]
#[table(name = "occupancies")]
pub struct Occupancy {
    pub appointment_id: AppointmentId,
    /// The room or provider whose time this blocks; or, instead, the client whose own time it blocks.
    pub bookable_id: Option<BookableId>,
    pub client_id: Option<ClientId>,
    pub starts_at: DateTime<Utc>,
    pub ends_at: DateTime<Utc>,
    pub engagement: Engagement,
    /// The status of the appointment, copied here so that seats are counted from this table alone.
    pub status: Status,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum Amount {
    Money(Price),
    Uses(u32),
}

/// What pays for an appointment, and who: a parent's insurance can pay for a child's appointment.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct Payment {
    #[schema(value_type = String, format = Uuid)]
    pub payer_id: ClientId,
    #[schema(value_type = String, format = Uuid)]
    pub method: PaymentMethodId,
    pub amount: Amount,
}

/// One client having one service, in one room with zero or more providers, from `starts_at` to `ends_at`. That is the
/// client's time; the time the room and the providers are blocked comes from the service.
#[derive(Debug, Serialize, Deserialize, Table, utoipa::ToSchema)]
#[table(name = "appointments")]
pub struct Appointment {
    #[schema(value_type = String, format = Uuid)]
    pub org_id: OrgId,
    #[schema(value_type = String, format = Uuid)]
    pub client_id: ClientId,
    #[schema(value_type = String, format = Uuid)]
    pub service_id: ServiceId,
    #[schema(value_type = Vec<String>)]
    pub provider_ids: Vec<BookableId>,
    #[schema(value_type = String, format = Uuid)]
    pub room_id: BookableId,
    pub starts_at: DateTime<Utc>,
    pub ends_at: DateTime<Utc>,
    pub status: Status,
    /// Only while `held`.
    pub expires_at: Option<DateTime<Utc>>,
    /// Not known while `held`; the payment is debited when the appointment becomes `booked`.
    pub payment: Option<Payment>,
    #[schema(value_type = Object)]
    pub attributes: Attributes,
}

/// `sqlx(json)` cannot read a payment that is SQL NULL, so the row is read by hand.
#[cfg(feature = "postgres")]
impl<'r> sqlx::FromRow<'r, sqlx::postgres::PgRow> for Appointment {
    fn from_row(row: &'r sqlx::postgres::PgRow) -> std::result::Result<Self, sqlx::Error> {
        use sqlx::types::Json;
        use sqlx::Row;
        Ok(Self {
            org_id: row.try_get("org_id")?,
            client_id: row.try_get("client_id")?,
            service_id: row.try_get("service_id")?,
            provider_ids: row.try_get("provider_ids")?,
            room_id: row.try_get("room_id")?,
            starts_at: row.try_get("starts_at")?,
            ends_at: row.try_get("ends_at")?,
            status: row.try_get("status")?,
            expires_at: row.try_get("expires_at")?,
            payment: row.try_get::<Option<Json<Payment>>, _>("payment")?.map(|json| json.0),
            attributes: row.try_get::<Json<Attributes>, _>("attributes")?.0,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "postgres", derive(sqlx::Type), sqlx(transparent))]
pub struct IdempotencyKey(pub String);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum Settlement {
    Prepaid { balance: Amount, valid: Option<Interval> },
    OnSpot,
}

/// Pass, voucher, coupon, insurance, cash, card. `owner` is empty for shared methods.
#[derive(Debug, Serialize, Deserialize, Table, utoipa::ToSchema, validator::Validate)]
#[cfg_attr(feature = "postgres", derive(sqlx::FromRow))]
#[table(name = "payment_methods")]
#[validate(context = "Vec<OrgId>")]
pub struct PaymentMethod {
    /// The org of the path, or one grouped under it.
    #[schema(value_type = String, format = Uuid)]
    #[validate(custom(function = "within_orgs", use_context))]
    pub org_id: OrgId,
    #[validate(custom(function = "not_blank"))]
    pub name: String,
    #[schema(value_type = Option<String>, format = Uuid)]
    pub owner_id: Option<ClientId>,
    #[cfg_attr(feature = "postgres", sqlx(json))]
    #[schema(value_type = Object)]
    pub eligible: Condition,
    #[cfg_attr(feature = "postgres", sqlx(json))]
    #[validate(custom(function = "valid_window"))]
    pub settlement: Settlement,
    #[cfg_attr(feature = "postgres", sqlx(json))]
    #[schema(value_type = Object)]
    pub attributes: Attributes,
}

fn valid_window(settlement: &Settlement) -> std::result::Result<(), validator::ValidationError> {
    match settlement {
        Settlement::Prepaid { valid: Some(window), .. } if window.end <= window.start => Err(validator::ValidationError::new("empty_window")),
        _ => Ok(()),
    }
}

/// Persisted forms. Soft delete, `created_at` and `updated_at` come from `Entity`.
pub type OccupancyRow = Entity<OccupancyId, Occupancy>;
pub type AppointmentRow = Entity<AppointmentId, Appointment>;
pub type PaymentMethodRow = Entity<PaymentMethodId, PaymentMethod>;

/// Slots that satisfy every need of every party with no conflict.
#[allow(async_fn_in_trait)]
pub trait Search {
    async fn find(&self, request: &SearchRequest) -> Result<Vec<Slot>>;

    /// The first day, in the org's time zone, with any slot after the window, looking at most `horizon_days` ahead.
    async fn next_day(&self, request: &SearchRequest, horizon_days: u32) -> Result<Option<chrono::NaiveDate>>;
}

#[allow(async_fn_in_trait)]
pub trait Appointments {
    /// Keeps the slot until `until` as `held` appointments, one for each placement. A hold needs capacity, never a
    /// waitlist place, and every party of the search needs a client.
    async fn hold(&self, request: &SearchRequest, slot: &Slot, until: DateTime<Utc>) -> Result<Vec<AppointmentRow>>;

    /// Books a `held` appointment and debits `payment`. The same key returns the same appointment.
    async fn book_held(&self, held: AppointmentId, payment: Payment, key: IdempotencyKey) -> Result<AppointmentRow>;

    async fn cancel(&self, appointment: AppointmentId) -> Result<()>;

    /// Books the slot directly, one appointment for each placement and `payments[i]` for placement `i`: `booked` while
    /// every room has capacity, `waitlist` when one is over capacity but within its waitlist, otherwise
    /// `Error::Occupied`. A payment is debited only when the appointment is `booked`. The same key and request return
    /// the same appointments.
    async fn book(&self, request: &SearchRequest, slot: &Slot, payments: Vec<Payment>, key: IdempotencyKey) -> Result<Vec<AppointmentRow>>;

    /// The only status changes: waitlist to booked (needs capacity; debits the payment) or cancelled, booked to confirmed
    /// or cancelled, confirmed to cancelled. Anything else is `Error::Transition`.
    async fn transition(&self, appointment: AppointmentId, to: Status) -> Result<AppointmentRow>;

    /// Moves the appointment to the one placement of `slot`; its own time counts for nothing while the new slot is
    /// checked, so it may overlap its old time.
    async fn reschedule(&self, appointment: AppointmentId, request: &SearchRequest, slot: &Slot) -> Result<AppointmentRow>;
}
