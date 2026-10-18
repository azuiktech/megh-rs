use std::collections::BTreeMap;
use std::fmt::Debug;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{Entity, Table};

macro_rules! ids {
    ($($name:ident),*) => {$(
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(transparent)]
        #[cfg_attr(feature = "postgres", derive(sqlx::Type), sqlx(transparent))]
        pub struct $name(pub Uuid);
    )*};
}

ids!(
    BookableId, ClientId, ServiceId, ScheduleId,
    AppointmentId, HoldId, OccupancyId, PaymentMethodId, WaitlistEntryId
);

/// A location, brand or franchise is an `Org`; its id is the org's `Uuid`.
pub type OrgId = Uuid;

/// Tags and metadata are this one map.
pub type Attributes = BTreeMap<String, serde_json::Value>;

/// A condition over the key/value map of an entity. Scriptable implementations come later.
#[typetag::serde]
pub trait Predicate: Debug + Send + Sync {
    fn holds(&self, subject: &Attributes) -> bool;
}

pub type Condition = Box<dyn Predicate>;

/// The entity kind a grouping edge connects: both ends are of this kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "postgres", derive(sqlx::Type), sqlx(type_name = "text", rename_all = "snake_case"))]
pub enum Grouped {
    Org,
    Bookable,
    Service,
    Client,
}

/// Grouping graph over any entity kind: brand, franchise, category, lobby, role.
/// A group rule applies to its members transitively; cycles are rejected.
/// (`org::Member` already means a user in an org, hence the name.)
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Table)]
#[cfg_attr(feature = "postgres", derive(sqlx::FromRow))]
#[table(name = "groupings")]
pub struct Grouping {
    pub org_id: Option<OrgId>,
    pub kind: Grouped,
    pub member_id: Uuid,
    pub group_id: Uuid,
}

/// A provider or a room. Capacity above 1 is double booking.
#[derive(Debug, Serialize, Deserialize, Table)]
#[cfg_attr(feature = "postgres", derive(sqlx::FromRow))]
#[table(name = "bookables")]
pub struct Bookable {
    pub org_id: OrgId,
    pub name: String,
    #[cfg_attr(feature = "postgres", sqlx(try_from = "i32"))]
    pub capacity: u32,
    #[cfg_attr(feature = "postgres", sqlx(json))]
    pub attributes: Attributes,
}

/// A person, pet or other; the kind is in the attributes.
#[derive(Debug, Serialize, Deserialize, Table)]
#[cfg_attr(feature = "postgres", derive(sqlx::FromRow))]
#[table(name = "clients")]
pub struct Client {
    pub org_id: Option<OrgId>,
    pub name: String,
    #[cfg_attr(feature = "postgres", sqlx(json))]
    pub attributes: Attributes,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "postgres", derive(sqlx::Type), sqlx(type_name = "text", rename_all = "snake_case"))]
pub enum Mode {
    Appointment,
    Class,
    Enrollment,
    Resource,
    Arrival,
}

/// Micros of an ISO 4217 currency, as stored; `money::from_micros` gives the `Money`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Price {
    pub micros: i64,
    pub currency: String,
}

/// A catalogue node. A group (category, menu) has no steps; its children are Groupings.
/// Add-ons are services in the same catalogue.
#[derive(Debug, Serialize, Deserialize, Table)]
#[cfg_attr(feature = "postgres", derive(sqlx::FromRow))]
#[table(name = "services")]
pub struct Service {
    pub org_id: Option<OrgId>,
    pub name: String,
    pub mode: Mode,
    #[cfg_attr(feature = "postgres", sqlx(json))]
    pub price: Price,
    #[cfg_attr(feature = "postgres", sqlx(json))]
    pub steps: Vec<Step>,
    #[cfg_attr(feature = "postgres", sqlx(json))]
    pub variants: Vec<Variant>,
    #[cfg_attr(feature = "postgres", sqlx(json))]
    pub attributes: Attributes,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Presence {
    Present,
    Absent,
}

/// Procedure codes and descriptors are in the attributes.
#[derive(Debug, Serialize, Deserialize)]
pub struct Step {
    pub duration: Duration,
    pub client: Presence,
    pub needs: Vec<Need>,
    pub attributes: Attributes,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "postgres", derive(sqlx::Type), sqlx(type_name = "text", rename_all = "snake_case"))]
pub enum Engagement {
    Engaged,
    Free,
    OnDemand,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Need {
    pub who: Condition,
    pub count: u32,
    pub engagement: Engagement,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Change<T> {
    Set(T),
    Add(T),
}

/// A delta on the whole service, resolved once provider and client are known.
#[derive(Debug, Serialize, Deserialize)]
pub struct Variant {
    pub when: Condition,
    pub duration: Option<Change<Duration>>,
    pub price_micros: Option<Change<i64>>,
}

/// Persisted forms. Soft delete, `created_at` and `updated_at` come from `Entity`.
pub type GroupingRow = Entity<Uuid, Grouping>;
pub type BookableRow = Entity<BookableId, Bookable>;
pub type ClientRow = Entity<ClientId, Client>;
pub type ServiceRow = Entity<ServiceId, Service>;

/// Ready-made services. Each returns an ordinary service of ordinary steps.
pub trait Templates: Sized {
    fn simple(name: &str, duration: Duration, provider: Condition, room: Condition) -> Self;
    fn with_cleanup(self, cleanup: Duration, room: Condition) -> Self;
    fn processed(
        name: &str,
        prep: Duration,
        wait: Duration,
        finish: Duration,
        provider: Condition,
        room: Condition,
    ) -> Self;
    fn handoff(name: &str, turns: Vec<(Condition, Duration)>, room: Condition) -> Self;
    fn room_only(name: &str, duration: Duration, room: Condition) -> Self;
    fn session(name: &str, duration: Duration, capacity: u32, room: Condition) -> Self;
}
