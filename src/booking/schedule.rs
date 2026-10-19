use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::{lasting, within_orgs, ScheduleId, BookableId, OrgId, Result, ServiceId};
use crate::{Entity, Table};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct Interval {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "postgres", derive(sqlx::Type), sqlx(type_name = "text", rename_all = "snake_case"))]
pub enum Kind {
    Open,
    Blockout,
}

/// An RFC 5545 recurrence (DTSTART and RRULE/EXDATE); it is read and written as that text.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(transparent)]
#[schema(value_type = String, example = "DTSTART;TZID=America/New_York:20261104T090000\nRRULE:FREQ=WEEKLY;BYDAY=MO,TU")]
pub struct Recurrence(pub rrule::RRuleSet);

impl std::str::FromStr for Recurrence {
    type Err = rrule::RRuleError;

    fn from_str(text: &str) -> std::result::Result<Self, Self::Err> {
        text.parse().map(Self)
    }
}

#[cfg(feature = "postgres")]
impl sqlx::Type<sqlx::Postgres> for Recurrence {
    fn type_info() -> sqlx::postgres::PgTypeInfo {
        <String as sqlx::Type<sqlx::Postgres>>::type_info()
    }
}

#[cfg(feature = "postgres")]
impl sqlx::Encode<'_, sqlx::Postgres> for Recurrence {
    fn encode_by_ref(&self, buf: &mut sqlx::postgres::PgArgumentBuffer) -> std::result::Result<sqlx::encode::IsNull, sqlx::error::BoxDynError> {
        <String as sqlx::Encode<sqlx::Postgres>>::encode(self.0.to_string(), buf)
    }
}

#[cfg(feature = "postgres")]
impl sqlx::Decode<'_, sqlx::Postgres> for Recurrence {
    fn decode(value: sqlx::postgres::PgValueRef<'_>) -> std::result::Result<Self, sqlx::error::BoxDynError> {
        Ok(<String as sqlx::Decode<sqlx::Postgres>>::decode(value)?.parse()?)
    }
}

/// How long each occurrence of a recurrence lasts; it is read and written as ISO 8601 text, such as `PT8H`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[schema(value_type = String, format = Duration, example = "PT8H")]
pub struct Span(#[serde(with = "jiff::fmt::serde::unsigned_duration::required")] pub Duration);

/// Open or blocked time on any combination of axes; an omitted axis means any.
/// Eligibility is this schedule: a combination with no matching Open rule cannot be booked.
#[derive(Clone, Debug, Serialize, Deserialize, Table, utoipa::ToSchema, validator::Validate)]
#[cfg_attr(feature = "postgres", derive(sqlx::FromRow))]
#[table(name = "schedules")]
#[validate(context = "Vec<OrgId>")]
pub struct Schedule {
    /// The org of the path, or one grouped under it.
    #[schema(value_type = String, format = Uuid)]
    #[validate(custom(function = "within_orgs", use_context))]
    pub org_id: OrgId,
    #[schema(value_type = Option<String>, format = Uuid)]
    pub service_id: Option<ServiceId>,
    #[schema(value_type = Option<String>, format = Uuid)]
    pub bookable_id: Option<BookableId>,
    pub kind: Kind,
    pub recurrence: Recurrence,
    /// ISO 8601, such as `PT8H`.
    #[cfg_attr(feature = "postgres", sqlx(json))]
    #[validate(custom(function = "lasting"))]
    pub span: Span,
    /// Why the time is blocked out; for blockouts.
    pub reason: Option<String>,
    pub notes: Option<String>,
}

pub type ScheduleRow = Entity<ScheduleId, Schedule>;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Axes {
    pub org: OrgId,
    pub service: ServiceId,
    pub bookable: BookableId,
}

/// Open minus Blockout for one combination. Open and Blockout are each taken from the nearest
/// level that defines them, inner to outer; with several parents, from the longest path.
#[allow(async_fn_in_trait)]
pub trait Availability {
    async fn open(&self, axes: &Axes, within: &Interval) -> Result<Vec<Interval>>;
}
