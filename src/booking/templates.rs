use std::time::Duration;

use super::{Attributes, Condition, Engagement, Mode, Need, Presence, Price, Service, Step, Templates};

/// A copy of a condition, through its serialized form: conditions are trait objects without `Clone`.
fn duplicate(condition: &Condition) -> Condition {
    let stored = serde_json::to_value(condition).expect("a condition serializes");
    serde_json::from_value(stored).expect("a condition deserializes")
}

fn need(who: Condition, engagement: Engagement) -> Need {
    Need { who, count: 1, engagement }
}

fn step(duration: Duration, client: Presence, needs: Vec<Need>) -> Step {
    Step { duration, client, needs, attributes: Attributes::new() }
}

fn present(duration: Duration, provider: (Condition, Engagement), room: &Condition) -> Step {
    step(duration, Presence::Present, vec![need(provider.0, provider.1), need(duplicate(room), Engagement::Engaged)])
}

fn service(name: &str, mode: Mode, steps: Vec<Step>) -> Service {
    let price = Price { micros: 0, currency: "USD".into() };
    Service { org_id: None, name: name.into(), mode, price, steps, variants: vec![], attributes: Attributes::new() }
}

impl Templates for Service {
    fn simple(name: &str, duration: Duration, provider: Condition, room: Condition) -> Self {
        service(name, Mode::Appointment, vec![present(duration, (provider, Engagement::Engaged), &room)])
    }

    fn with_cleanup(mut self, cleanup: Duration, room: Condition) -> Self {
        self.steps.push(step(cleanup, Presence::Absent, vec![need(room, Engagement::Engaged)]));
        self
    }

    fn processed(name: &str, prep: Duration, wait: Duration, finish: Duration, provider: Condition, room: Condition) -> Self {
        let steps = vec![
            present(prep, (duplicate(&provider), Engagement::Engaged), &room),
            present(wait, (duplicate(&provider), Engagement::Free), &room),
            present(finish, (provider, Engagement::Engaged), &room),
        ];
        service(name, Mode::Appointment, steps)
    }

    fn handoff(name: &str, turns: Vec<(Condition, Duration)>, room: Condition) -> Self {
        let steps = turns.into_iter().map(|(provider, duration)| present(duration, (provider, Engagement::Engaged), &room)).collect();
        service(name, Mode::Appointment, steps)
    }

    fn room_only(name: &str, duration: Duration, room: Condition) -> Self {
        service(name, Mode::Resource, vec![step(duration, Presence::Present, vec![need(room, Engagement::Engaged)])])
    }

    fn session(name: &str, duration: Duration, capacity: u32, room: Condition) -> Self {
        let mut session = Self::room_only(name, duration, room);
        session.mode = Mode::Class;
        session.attributes.insert("capacity".into(), capacity.into());
        session
    }
}
