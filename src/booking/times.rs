//! Open time as a set of ranges, with the set arithmetic of the `rangemap` crate.

use std::ops::Range;

use chrono::{DateTime, Utc};
use rangemap::RangeSet;

use super::Interval;

pub(crate) type Times = RangeSet<DateTime<Utc>>;

fn range(interval: &Interval) -> Range<DateTime<Utc>> {
    interval.start..interval.end
}

pub(crate) fn times<'a>(intervals: impl IntoIterator<Item = &'a Interval>) -> Times {
    intervals.into_iter().map(range).collect()
}

pub(crate) fn intervals(times: &Times) -> Vec<Interval> {
    times.iter().map(|range| Interval { start: range.start, end: range.end }).collect()
}

pub(crate) fn covers(times: &Times, needed: &Interval) -> bool {
    times.gaps(&range(needed)).next().is_none()
}

pub(crate) fn overlap(left: &Interval, right: &Interval) -> bool {
    times([left]).overlaps(&range(right))
}
