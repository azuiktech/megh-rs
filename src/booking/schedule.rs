use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::{ScheduleId, BookableId, OrgId, Result, ServiceId};
use crate::{Entity, Table};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct Interval {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "postgres", derive(sqlx::Type), sqlx(type_name = "text", rename_all = "snake_case"))]
pub enum Kind {
    Open,
    Blockout,
}

/// RFC 5545 recurrence text (DTSTART and RRULE/EXDATE). The rrule library is not yet chosen.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "postgres", derive(sqlx::Type), sqlx(transparent))]
pub struct Recurrence(pub String);

/// Open or blocked time on any combination of axes; an omitted axis means any.
/// Eligibility is this schedule: a combination with no matching Open rule cannot be booked.
#[derive(Clone, Debug, Serialize, Deserialize, Table)]
#[cfg_attr(feature = "postgres", derive(sqlx::FromRow))]
#[table(name = "schedules")]
pub struct Schedule {
    pub org_id: OrgId,
    pub service_id: Option<ServiceId>,
    pub bookable_id: Option<BookableId>,
    pub kind: Kind,
    pub recurrence: Recurrence,
    #[cfg_attr(feature = "postgres", sqlx(json))]
    pub span: Duration,
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
