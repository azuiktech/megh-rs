use std::fmt::Debug;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{Entity, Table};

macro_rules! ids {
    ($($name:ident),*) => {$(
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, utoipa::ToSchema)]
        #[serde(transparent)]
        #[schema(value_type = String, format = Uuid)]
        #[cfg_attr(feature = "postgres", derive(sqlx::Type), sqlx(transparent))]
        pub struct $name(pub Uuid);
    )*};
}

ids!(
    BookableId, ClientId, ServiceId, ScheduleId,
    AppointmentId, OccupancyId, PaymentMethodId, GroupingId
);

/// A location, brand or franchise is an `Org`; its id is the org's `Uuid`.
pub type OrgId = Uuid;

/// Tags and metadata are this one map.
/// Assorted fields of an entity: any JSON, usually an object.
pub type Attributes = serde_json::Value;

/// A condition over the key/value map of an entity. Scriptable implementations come later.
#[typetag::serde]
pub trait Predicate: Debug + Send + Sync {
    fn holds(&self, subject: &Attributes) -> bool;
}

pub type Condition = Box<dyn Predicate>;

/// The entity kind a grouping edge connects: both ends are of this kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
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
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Table, utoipa::ToSchema)]
#[cfg_attr(feature = "postgres", derive(sqlx::FromRow))]
#[table(name = "groupings")]
pub struct Grouping {
    /// The org of the path.
    #[schema(value_type = String, format = Uuid)]
    pub org_id: OrgId,
    pub kind: Grouped,
    pub member_id: Uuid,
    pub group_id: Uuid,
}

fn not_blank(text: &str) -> Result<(), validator::ValidationError> {
    if text.trim().is_empty() { Err(validator::ValidationError::new("blank")) } else { Ok(()) }
}

fn within_orgs(org: &OrgId, orgs: &Vec<OrgId>) -> Result<(), validator::ValidationError> {
    if orgs.contains(org) { Ok(()) } else { Err(validator::ValidationError::new("outside_orgs")) }
}

/// A provider or a room. Capacity above 1 is double booking.
#[derive(Debug, Serialize, Deserialize, Table, utoipa::ToSchema, validator::Validate)]
#[cfg_attr(feature = "postgres", derive(sqlx::FromRow))]
#[table(name = "bookables")]
#[validate(context = "Vec<OrgId>")]
pub struct Bookable {
    /// The org of the path, or one grouped under it.
    #[schema(value_type = String, format = Uuid)]
    #[validate(custom(function = "within_orgs", use_context))]
    pub org_id: OrgId,
    #[validate(custom(function = "not_blank"))]
    pub name: String,
    #[cfg_attr(feature = "postgres", sqlx(try_from = "i32"))]
    #[validate(range(min = 1))]
    pub capacity: u32,
    /// Places for appointments beyond `capacity`; they are `waitlist` appointments.
    #[cfg_attr(feature = "postgres", sqlx(try_from = "i32"))]
    #[serde(default)]
    pub waitlist: u32,
    #[cfg_attr(feature = "postgres", sqlx(json))]
    #[schema(value_type = Object)]
    pub attributes: Attributes,
}

/// A person, pet or other; the kind is in the attributes.
#[derive(Debug, Serialize, Deserialize, Table, utoipa::ToSchema, validator::Validate)]
#[cfg_attr(feature = "postgres", derive(sqlx::FromRow))]
#[table(name = "clients")]
#[validate(context = "Vec<OrgId>")]
pub struct Client {
    /// The org of the path, or one grouped under it.
    #[schema(value_type = String, format = Uuid)]
    #[validate(custom(function = "within_orgs", use_context))]
    pub org_id: OrgId,
    #[validate(custom(function = "not_blank"))]
    pub name: String,
    #[validate(custom(function = "not_blank"))]
    pub first_name: String,
    #[validate(custom(function = "not_blank"))]
    pub last_name: String,
    pub email: Option<String>,
    pub home_phone: Option<String>,
    pub work_phone: Option<String>,
    pub mobile_phone: Option<String>,
    #[cfg_attr(feature = "postgres", sqlx(json))]
    #[schema(value_type = Object)]
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
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
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
    pub org_id: OrgId,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "postgres", derive(sqlx::Type), sqlx(type_name = "text", rename_all = "snake_case"))]
pub enum Engagement {
    Engaged,
    Free,
    OnDemand,
}

/// What a need is for: the room of an appointment, or a provider.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    #[default]
    Provider,
    Room,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Need {
    pub who: Condition,
    pub count: u32,
    pub engagement: Engagement,
    #[serde(default)]
    pub role: Role,
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
pub type GroupingRow = Entity<GroupingId, Grouping>;
pub type BookableRow = Entity<BookableId, Bookable>;
pub type ClientRow = Entity<ClientId, Client>;
pub type ServiceRow = Entity<ServiceId, Service>;

/// Ready-made services. Each returns an ordinary service of ordinary steps.
pub trait Templates: Sized {
    fn simple(org: OrgId, name: &str, duration: Duration, provider: Condition, room: Condition) -> Self;
    fn with_cleanup(self, cleanup: Duration, room: Condition) -> Self;
    fn processed(
        org: OrgId,
        name: &str,
        prep: Duration,
        wait: Duration,
        finish: Duration,
        provider: Condition,
        room: Condition,
    ) -> Self;
    fn handoff(org: OrgId, name: &str, turns: Vec<(Condition, Duration)>, room: Condition) -> Self;
    fn room_only(org: OrgId, name: &str, duration: Duration, room: Condition) -> Self;
    fn session(org: OrgId, name: &str, duration: Duration, capacity: u32, room: Condition) -> Self;
}
