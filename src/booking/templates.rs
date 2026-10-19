use std::time::Duration;

use super::{Condition, Engagement, Mode, Need, OrgId, Presence, Price, Service, Step, Templates};

/// A copy of a condition, through its serialized form: conditions are trait objects without `Clone`.
fn duplicate(condition: &Condition) -> Condition {
    let stored = serde_json::to_value(condition).expect("a condition serializes");
    serde_json::from_value(stored).expect("a condition deserializes")
}

fn need(who: Condition, engagement: Engagement) -> Need {
    Need { who, count: 1, engagement }
}

fn step(duration: Duration, client: Presence, needs: Vec<Need>) -> Step {
    Step { duration, client, needs, attributes: serde_json::json!({}) }
}

fn present(duration: Duration, provider: (Condition, Engagement), room: &Condition) -> Step {
    step(duration, Presence::Present, vec![need(provider.0, provider.1), need(duplicate(room), Engagement::Engaged)])
}

fn service(org: OrgId, name: &str, mode: Mode, steps: Vec<Step>) -> Service {
    let price = Price { micros: 0, currency: "USD".into() };
    Service { org_id: org, name: name.into(), mode, price, steps, variants: vec![], attributes: serde_json::json!({}) }
}

impl Templates for Service {
    fn simple(org: OrgId, name: &str, duration: Duration, provider: Condition, room: Condition) -> Self {
        service(org, name, Mode::Appointment, vec![present(duration, (provider, Engagement::Engaged), &room)])
    }

    fn with_cleanup(mut self, cleanup: Duration, room: Condition) -> Self {
        self.steps.push(step(cleanup, Presence::Absent, vec![need(room, Engagement::Engaged)]));
        self
    }

    fn processed(org: OrgId, name: &str, prep: Duration, wait: Duration, finish: Duration, provider: Condition, room: Condition) -> Self {
        let steps = vec![
            present(prep, (duplicate(&provider), Engagement::Engaged), &room),
            present(wait, (duplicate(&provider), Engagement::Free), &room),
            present(finish, (provider, Engagement::Engaged), &room),
        ];
        service(org, name, Mode::Appointment, steps)
    }

    fn handoff(org: OrgId, name: &str, turns: Vec<(Condition, Duration)>, room: Condition) -> Self {
        let steps = turns.into_iter().map(|(provider, duration)| present(duration, (provider, Engagement::Engaged), &room)).collect();
        service(org, name, Mode::Appointment, steps)
    }

    fn room_only(org: OrgId, name: &str, duration: Duration, room: Condition) -> Self {
        service(org, name, Mode::Resource, vec![step(duration, Presence::Present, vec![need(room, Engagement::Engaged)])])
    }

    fn session(org: OrgId, name: &str, duration: Duration, capacity: u32, room: Condition) -> Self {
        let mut session = Self::room_only(org, name, duration, room);
        session.mode = Mode::Class;
        session.attributes["capacity"] = capacity.into();
        session
    }
}
