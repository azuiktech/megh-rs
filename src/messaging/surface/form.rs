//! Ready-made surfaces the user fills in; each has a `submit` button carrying the context its answer reads.

use serde_json::{Value, json};
use uuid::Uuid;

use super::Surface;
use super::component::{self as ui, Group, bound};
use crate::booking::availability::Slot;
use crate::booking::value::Name;

#[derive(Debug, Clone, PartialEq)]
pub struct Prompt {
    pub text: String,
}

impl Prompt {
    pub fn new(text: impl Into<String>) -> Self {
        Self { text: text.into() }
    }
}

/// Something to pick: a product, a service, a provider.
#[derive(Debug, Clone, PartialEq)]
pub struct Choice {
    pub id: Uuid,
    pub label: String,
}

impl Choice {
    pub fn new(id: Uuid, label: impl Into<String>) -> Self {
        Self { id, label: label.into() }
    }
}

/// A rating from 1 to `max`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Scale {
    pub max: u32,
}

impl Scale {
    pub fn new(max: u32) -> Self {
        Self { max }
    }
}

impl Surface {
    pub fn selection(prompt: Prompt, choices: Vec<Choice>) -> Self {
        let options: Vec<Value> = choices.iter().map(|c| json!({"label": c.label, "value": c.id})).collect();
        let picker = json!({
            "id": "choices", "component": "ChoicePicker", "variant": "mutuallyExclusive",
            "options": options, "value": bound("/picked"),
        });
        let context = json!({"selected": [{"id": bound("/picked/0")}]});
        ui::surface(&prompt.text, vec![vec![picker], ui::submit("submit", "Send", context)], json!({"picked": []}))
    }

    pub fn quantities(prompt: Prompt, choices: Vec<Choice>) -> Self {
        let rows: Vec<Group> = choices.iter().enumerate().map(|(n, choice)| quantity(n, choice)).collect();
        let selected: Vec<Value> = choices
            .iter()
            .enumerate()
            .map(|(n, choice)| json!({"id": choice.id, "count": bound(&format!("/counts/{n}"))}))
            .collect();
        let groups = vec![ui::container("Column", "rows", rows), ui::submit("submit", "Send", json!({"selected": selected}))];
        ui::surface(&prompt.text, groups, json!({"counts": vec![0; choices.len()]}))
    }

    pub fn login(prompt: Prompt) -> Self {
        let action = json!({"functionCall": {"@call": "openUrl", "args": {"url": "/auth/login"}}});
        ui::surface(&prompt.text, vec![ui::button("login", "Sign in", action)], json!({}))
    }

    pub fn logout(prompt: Prompt) -> Self {
        let action = json!({"functionCall": {"@call": "openUrl", "args": {"url": "/auth/logout"}}});
        ui::surface(&prompt.text, vec![ui::button("logout", "Sign out", action)], json!({}))
    }

    pub fn open_times(prompt: Prompt, slots: Vec<Slot>) -> Self {
        let times: Vec<Group> = slots
            .iter()
            .filter_map(|slot| slot.parties.first())
            .enumerate()
            .map(|(n, party)| {
                let slot = json!({"starts_at": party.starts_at, "ends_at": party.ends_at, "service_id": party.service_id});
                let label = party.starts_at.format("%a %d %b %H:%M").to_string();
                ui::submit(&format!("time-{n}"), &label, json!({"slot": slot}))
            })
            .collect();
        ui::surface(&prompt.text, vec![ui::container("Column", "times", times)], json!({}))
    }

    pub fn calendar_date(prompt: Prompt) -> Self {
        let groups = vec![ui::date("from", "Date", "/from"), ui::submit("submit", "Send", json!({"from": bound("/from")}))];
        ui::surface(&prompt.text, groups, json!({"from": ""}))
    }

    pub fn calendar_range(prompt: Prompt) -> Self {
        let context = json!({"from": bound("/from"), "to": bound("/to")});
        let groups = vec![
            ui::date("from", "From", "/from"),
            ui::date("to", "To", "/to"),
            ui::submit("submit", "Send", context),
        ];
        ui::surface(&prompt.text, groups, json!({"from": "", "to": ""}))
    }

    pub fn intake(prompt: Prompt) -> Self {
        const FIELDS: [(&str, &str); 5] = [
            ("first_name", "First name"),
            ("last_name", "Last name"),
            ("name", "Name"),
            ("email", "Email"),
            ("mobile_phone", "Mobile phone"),
        ];
        let inputs = FIELDS.iter().map(|(key, label)| ui::field(key, label, &format!("/{key}")));
        let context: serde_json::Map<String, Value> =
            FIELDS.iter().map(|(key, _)| (key.to_string(), bound(&format!("/{key}")))).collect();
        let groups = inputs.chain([ui::submit("submit", "Send", Value::Object(context))]).collect();
        ui::surface(&prompt.text, groups, json!({}))
    }

    pub fn feedback_rating(prompt: Prompt, scale: Scale) -> Self {
        let slider = json!({
            "id": "rating", "component": "Slider", "label": "Rating",
            "min": 1, "max": scale.max, "steps": scale.max.saturating_sub(1), "value": bound("/rating"),
        });
        let groups = vec![vec![slider], ui::submit("submit", "Send", json!({"rating": bound("/rating")}))];
        ui::surface(&prompt.text, groups, json!({"rating": scale.max.div_ceil(2)}))
    }

    pub fn feedback_choice(prompt: Prompt, labels: Vec<Name>) -> Self {
        let options: Vec<Value> = labels.iter().map(|l| json!({"label": l.as_ref(), "value": l.as_ref()})).collect();
        let picker = json!({
            "id": "choice", "component": "ChoicePicker", "variant": "mutuallyExclusive",
            "options": options, "value": bound("/choice"),
        });
        let groups = vec![vec![picker], ui::submit("submit", "Send", json!({"choice": bound("/choice/0")}))];
        ui::surface(&prompt.text, groups, json!({"choice": []}))
    }

    pub fn approval(prompt: Prompt, fields: Vec<(Name, String)>) -> Self {
        let lines: Vec<Group> = fields
            .iter()
            .enumerate()
            .map(|(n, (name, value))| ui::text(&format!("field-{n}"), &format!("{name}: {value}")))
            .collect();
        let groups = vec![
            ui::container("Column", "fields", lines),
            ui::submit("approve", "Approve", json!({"decision": "approved"})),
            ui::submit("reject", "Reject", json!({"decision": "rejected"})),
        ];
        ui::surface(&prompt.text, groups, json!({}))
    }
}

fn quantity(n: usize, choice: &Choice) -> Group {
    let slider = json!({
        "id": format!("count-{n}"), "component": "Slider", "label": choice.label,
        "min": 0, "max": 99, "steps": 99, "value": bound(&format!("/counts/{n}")),
    });
    vec![slider]
}
