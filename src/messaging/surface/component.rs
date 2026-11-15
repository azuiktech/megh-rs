//! Basic-catalog components as JSON, and the createSurface message that carries them.

use std::iter::once;

use serde_json::{Value, json};
use uuid::Uuid;

use super::Surface;

const CATALOG_ID: &str = "https://a2ui.org/specification/v1_0/catalogs/basic/catalog.json";

pub(super) type Group = Vec<Value>;

pub(super) fn text(id: &str, text: &str) -> Group {
    vec![json!({"id": id, "component": "Text", "text": text})]
}

pub(super) fn bound(path: &str) -> Value {
    json!({"@path": path})
}

pub(super) fn event(name: &str, context: Value) -> Value {
    json!({"event": {"name": name, "context": context}})
}

pub(super) fn button(id: &str, label: &str, action: Value) -> Group {
    let label_id = format!("{id}-label");
    let button = json!({"id": id, "component": "Button", "child": label_id, "action": action});
    once(button).chain(text(&label_id, label)).collect()
}

pub(super) fn submit(id: &str, label: &str, context: Value) -> Group {
    button(id, label, event("submit", context))
}

pub(super) fn field(id: &str, label: &str, path: &str) -> Group {
    vec![json!({"id": id, "component": "TextField", "label": label, "value": bound(path)})]
}

pub(super) fn date(id: &str, label: &str, path: &str) -> Group {
    vec![json!({"id": id, "component": "DateTimeInput", "label": label, "enableDate": true, "value": bound(path)})]
}

pub(super) fn container(kind: &str, id: &str, groups: Vec<Group>) -> Group {
    let children: Vec<&Value> = groups.iter().filter_map(|group| group.first()).map(|head| &head["id"]).collect();
    let head = json!({"id": id, "component": kind, "children": children});
    once(head).chain(groups.into_iter().flatten()).collect()
}

pub(super) fn surface(lead: &str, groups: Vec<Group>, data: Value) -> Surface {
    let id = Uuid::now_v7().to_string();
    let components = container("Column", "root", once(text("lead", lead)).chain(groups).collect());
    let message = json!({
        "version": "1.0",
        "createSurface": {"surfaceId": id, "catalogId": CATALOG_ID, "components": components, "dataModel": data},
    });
    Surface { id, messages: vec![message] }
}
