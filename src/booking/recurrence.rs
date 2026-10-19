use std::time::Duration;

use chrono::{DateTime, Utc};
use rrule::Tz;

use super::times::{times, Times};
use super::{Error, Interval, Recurrence, Result};

fn zoned(moment: DateTime<Utc>) -> DateTime<Tz> {
    moment.with_timezone(&Tz::UTC)
}

fn invalid(error: impl ToString) -> Error {
    Error::Recurrence(error.to_string())
}

impl Recurrence {
    /// The occurrences of `span` length that touch `within`, clipped to it. The text is RFC 5545
    /// (`DTSTART;TZID=America/New_York:...` keeps local time across daylight saving).
    pub(crate) fn within(&self, span: Duration, within: &Interval) -> Result<Times> {
        let length = chrono::Duration::from_std(span).map_err(invalid)?;
        let starts = self.0.clone().after(zoned(within.start - length)).before(zoned(within.end)).all(u16::MAX).dates;
        let occurrences: Vec<_> = starts.into_iter().map(|start| start.with_timezone(&Utc)).map(|start| Interval { start, end: start + length }).collect();
        let window = times([within]);
        Ok(times(&occurrences).intersection(&window).collect())
    }
}

