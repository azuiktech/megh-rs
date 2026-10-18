use std::collections::HashMap;

use sqlx::PgPool;
use uuid::Uuid;

use super::times::{intervals, Times};
use super::{Schedule, ScheduleRow, Axes, Groups, Grouped, Interval, Kind, Result};

type Reach = HashMap<Uuid, usize>;
type Rank = (usize, usize, usize);

/// The groups above the bookable, the org and the service of a combination.
struct Above {
    bookable: Reach,
    org: Reach,
    service: Reach,
}

/// Every live schedule and blockout and the grouping graphs they are resolved through.
pub(crate) struct Rules {
    rows: Vec<ScheduleRow>,
    orgs: Groups,
    bookables: Groups,
    services: Groups,
}

/// How near a rule's axis is to the combination: 0 is the thing itself, an omitted axis is the outermost.
fn nearness(axis: Option<Uuid>, reach: &Reach) -> Option<usize> {
    axis.map_or(Some(usize::MAX), |id| reach.get(&id).copied())
}

fn rank(rule: &Schedule, above: &Above) -> Option<Rank> {
    Some((
        nearness(rule.bookable_id.map(|id| id.0), &above.bookable)?,
        nearness(rule.org_id, &above.org)?,
        nearness(rule.service_id.map(|id| id.0), &above.service)?,
    ))
}

impl Rules {
    pub(crate) async fn load(pool: &PgPool) -> Result<Self> {
        let rows = sqlx::query_as("SELECT * FROM schedules WHERE deleted_at IS NULL").fetch_all(pool).await?;
        Ok(Self {
            rows,
            orgs: Groups::load(pool, Grouped::Org).await?,
            bookables: Groups::load(pool, Grouped::Bookable).await?,
            services: Groups::load(pool, Grouped::Service).await?,
        })
    }

    fn above(&self, axes: &Axes) -> Result<Above> {
        Ok(Above { bookable: self.bookables.reach(axes.bookable.0)?, org: self.orgs.reach(axes.org)?, service: self.services.reach(axes.service.0)? })
    }

    /// Schedules minus blockouts. The schedules come from the nearest level that defines any; every
    /// matching blockout applies, whatever its level.
    pub(crate) fn open(&self, axes: &Axes, within: &Interval) -> Result<Vec<Interval>> {
        Ok(intervals(&self.times(axes, within)?))
    }

    pub(crate) fn times(&self, axes: &Axes, within: &Interval) -> Result<Times> {
        let above = self.above(axes)?;
        let ranked = |kind: Kind| -> Vec<_> { self.rows.iter().filter(|row| row.kind == kind).filter_map(|row| Some((rank(row, &above)?, &row.data))).collect() };
        let schedules = ranked(Kind::Open);
        let nearest = schedules.iter().map(|(rank, _)| *rank).min();
        let open = schedules.iter().filter(|(rank, _)| Some(*rank) == nearest).map(|(_, rule)| *rule);
        let blockouts = occurrences(ranked(Kind::Blockout).into_iter().map(|(_, rule)| rule), within)?;
        Ok(blockouts.iter().fold(occurrences(open, within)?, |mut open, blockout| {
            open.remove(blockout.clone());
            open
        }))
    }
}

fn occurrences<'a>(rules: impl Iterator<Item = &'a Schedule>, within: &Interval) -> Result<Times> {
    let found: Result<Vec<_>> = rules.map(|rule| rule.recurrence.within(rule.span, within)).collect();
    Ok(found?.iter().flat_map(|times| times.iter().cloned()).collect())
}
