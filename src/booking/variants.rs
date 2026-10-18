use std::time::Duration;

use super::{Attributes, Change, Service};

/// The step durations and the price of a service once its variants are applied for the people involved.
/// A variant applies when its condition holds for any subject. A duration change lands on the first step.
pub(crate) fn resolve(service: &Service, subjects: &[&Attributes]) -> (Vec<Duration>, i64) {
    let applying = service.variants.iter().filter(|variant| subjects.iter().any(|subject| variant.when.holds(subject)));
    let base = (service.steps.iter().map(|step| step.duration).collect(), service.price.micros);
    applying.fold(base, |(mut durations, price), variant| {
        if let (Some(change), Some(first)) = (&variant.duration, durations.first().copied()) {
            let total: Duration = durations.iter().sum();
            durations[0] = match change {
                Change::Add(extra) => first + *extra,
                Change::Set(wanted) => (first + *wanted).saturating_sub(total),
            };
        }
        let price = match variant.price_micros {
            Some(Change::Add(extra)) => price + extra,
            Some(Change::Set(wanted)) => wanted,
            None => price,
        };
        (durations, price)
    })
}
