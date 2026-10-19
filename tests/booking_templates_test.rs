//! Specification: docs/TPD-booking.md, section 15 (templates) and section 14 (scenarios a, b, f, i).

use std::time::Duration;

use megh::booking::{Attributes, Condition, Engagement, Mode, Predicate, Presence, Service, Step, Templates};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
struct Is {
    key: String,
    value: String,
}

#[typetag::serde]
impl Predicate for Is {
    fn holds(&self, subject: &Attributes) -> bool {
        subject.get(&self.key).and_then(|value| value.as_str()) == Some(self.value.as_str())
    }
}

fn is(key: &str, value: &str) -> Condition {
    Box::new(Is { key: key.into(), value: value.into() })
}

fn attributes(key: &str, value: &str) -> Attributes {
    serde_json::json!({ key: value })
}

fn stylist() -> Attributes { attributes("role", "stylist") }
fn hygienist() -> Attributes { attributes("role", "hygienist") }
fn dentist() -> Attributes { attributes("role", "dentist") }
fn chair() -> Attributes { attributes("kind", "chair") }

fn org() -> uuid::Uuid {
    uuid::Uuid::nil()
}

fn minutes(count: u64) -> Duration {
    Duration::from_secs(count * 60)
}

/// The engagement of the one need in the step that selects `who`; `None` when no need selects it.
fn engagement(step: &Step, who: &Attributes) -> Option<Engagement> {
    let matching: Vec<_> = step.needs.iter().filter(|need| need.who.holds(who)).collect();
    assert!(matching.len() <= 1, "more than one need selects {who:?}");
    matching.first().map(|need| {
        assert_eq!(need.count, 1);
        need.engagement
    })
}

fn durations(service: &Service) -> Vec<Duration> {
    service.steps.iter().map(|step| step.duration).collect()
}

fn simple() -> Service {
    Service::simple(org(), "Consultation", minutes(60), is("role", "stylist"), is("kind", "chair"))
}

fn colour() -> Service {
    Service::processed(org(), "Hair colour", minutes(10), minutes(30), minutes(20), is("role", "stylist"), is("kind", "chair"))
}

fn dental() -> Service {
    Service::handoff(org(), 
        "Dental procedure",
        vec![
            (is("role", "hygienist"), minutes(15)),
            (is("role", "dentist"), minutes(30)),
            (is("role", "hygienist"), minutes(20)),
        ],
        is("kind", "chair"),
    )
}

#[test]
fn simple_is_one_appointment_step_of_the_given_duration() {
    let service = simple();
    assert_eq!(service.name, "Consultation");
    assert_eq!(service.mode, Mode::Appointment);
    assert_eq!(durations(&service), vec![minutes(60)]);
    assert_eq!(service.steps[0].client, Presence::Present);
}

#[test]
fn simple_engages_one_provider_and_one_room() {
    let step = &simple().steps[0];
    assert_eq!(step.needs.len(), 2);
    assert_eq!(engagement(step, &stylist()), Some(Engagement::Engaged));
    assert_eq!(engagement(step, &chair()), Some(Engagement::Engaged));
}

#[test]
fn cleanup_follows_the_service_step_unchanged() {
    let service = simple().with_cleanup(minutes(10), is("kind", "chair"));
    assert_eq!(durations(&service), vec![minutes(60), minutes(10)]);
    assert_eq!(service.steps[0].client, Presence::Present);
    assert_eq!(engagement(&service.steps[0], &stylist()), Some(Engagement::Engaged));
}

#[test]
fn cleanup_holds_only_the_room_while_the_client_is_gone() {
    let cleanup = &simple().with_cleanup(minutes(10), is("kind", "chair")).steps[1];
    assert_eq!(cleanup.client, Presence::Absent);
    assert_eq!(engagement(cleanup, &chair()), Some(Engagement::Engaged));
    assert_eq!(engagement(cleanup, &stylist()), None);
}

#[test]
fn processed_has_prep_wait_and_finish_steps() {
    assert_eq!(durations(&colour()), vec![minutes(10), minutes(30), minutes(20)]);
    assert_eq!(colour().mode, Mode::Appointment);
}

#[test]
fn processed_engages_the_provider_for_prep_and_finish() {
    let service = colour();
    assert_eq!(engagement(&service.steps[0], &stylist()), Some(Engagement::Engaged));
    assert_eq!(engagement(&service.steps[2], &stylist()), Some(Engagement::Engaged));
}

#[test]
fn processed_frees_the_provider_while_the_colour_dries() {
    assert_eq!(engagement(&colour().steps[1], &stylist()), Some(Engagement::Free));
}

#[test]
fn processed_keeps_the_client_and_the_room_throughout() {
    colour().steps.iter().for_each(|step| {
        assert_eq!(step.client, Presence::Present);
        assert_eq!(engagement(step, &chair()), Some(Engagement::Engaged));
    });
}

#[test]
fn handoff_has_one_step_per_turn_in_order() {
    assert_eq!(durations(&dental()), vec![minutes(15), minutes(30), minutes(20)]);
}

#[test]
fn handoff_engages_each_turns_provider_and_nobody_else() {
    let service = dental();
    let turns = [(&hygienist(), &dentist()), (&dentist(), &hygienist()), (&hygienist(), &dentist())];
    service.steps.iter().zip(turns).for_each(|(step, (engaged, absent))| {
        assert_eq!(engagement(step, engaged), Some(Engagement::Engaged));
        assert_eq!(engagement(step, absent), None);
    });
}

#[test]
fn handoff_keeps_the_client_and_the_room_throughout() {
    dental().steps.iter().for_each(|step| {
        assert_eq!(step.client, Presence::Present);
        assert_eq!(engagement(step, &chair()), Some(Engagement::Engaged));
    });
}

#[test]
fn room_only_needs_no_provider() {
    let service = Service::room_only(org(), "Float", minutes(60), is("kind", "chair"));
    assert_eq!(durations(&service), vec![minutes(60)]);
    assert_eq!(service.steps[0].needs.len(), 1);
    assert_eq!(engagement(&service.steps[0], &chair()), Some(Engagement::Engaged));
}

#[test]
fn session_is_a_class_of_one_step_in_a_room() {
    let service = Service::session(org(), "Music class", minutes(60), 20, is("kind", "chair"));
    assert_eq!(service.mode, Mode::Class);
    assert_eq!(durations(&service), vec![minutes(60)]);
    assert_eq!(engagement(&service.steps[0], &chair()), Some(Engagement::Engaged));
}
