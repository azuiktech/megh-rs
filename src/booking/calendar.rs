use super::rules::Rules;
use super::{Axes, Calendar, Interval, Result, Availability};

impl Availability for Calendar {
    async fn open(&self, axes: &Axes, within: &Interval) -> Result<Vec<Interval>> {
        Rules::load(&self.pool).await?.open(axes, within)
    }
}
