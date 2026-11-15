//! Surface: a fluent builder of a2ui 1.0 messages.

use itertools::Itertools;
use serde::Serialize;
use serde_json::{Map, Value, json};

const CATALOG: &str = "https://a2ui.org/specification/v1_0/catalogs/basic/catalog.json";
const VERSION: &str = "1.0";

/// How feedback is given: a rating from 1 to the number of stars, or one of a few words.
pub enum Scale {
    Stars(u8),
    Words(&'static [&'static str]),
}

/// A JSON schema of an object, whose properties become a form's inputs.
#[derive(Debug, Clone, PartialEq)]
pub struct Schema(Value);

impl Schema {
    /// The schema of an entity, as its `ToSchema` gives it, with every reference resolved.
    #[cfg(feature = "axum")]
    pub fn of<T: utoipa::ToSchema>() -> Self {
        let mut named = Vec::new();
        T::schemas(&mut named);
        let refs = named.into_iter().map(|(name, schema)| (name, json!(schema))).collect::<Map<_, _>>();
        Self(resolved(&json!(T::schema()), &refs))
    }

    /// The schema without the named properties.
    pub fn without(self, names: &[&str]) -> Self {
        let mut schema = self.0;
        schema.get_mut("properties").and_then(Value::as_object_mut).into_iter().for_each(|properties| properties.retain(|name, _| !names.contains(&name.as_str())));
        Self(schema)
    }
}

impl From<Value> for Schema {
    fn from(schema: Value) -> Self {
        Self(resolved(&schema, &Map::new()))
    }
}

/// The schema as one object: references followed, `allOf` merged, and a value that may be null taken as its type.
fn resolved(schema: &Value, refs: &Map<String, Value>) -> Value {
    let named = schema["$ref"].as_str().and_then(|path| refs.get(path.rsplit('/').next()?));
    let parts = ["allOf", "oneOf", "anyOf"].iter().find_map(|key| schema[key].as_array());
    match (named, parts) {
        (Some(target), _) => resolved(target, refs),
        (None, Some(parts)) => parts.iter().map(|part| resolved(part, refs)).filter(|part| part["type"] != "null").fold(json!({}), joined),
        (None, None) => plain(schema, refs),
    }
}

fn plain(schema: &Value, refs: &Map<String, Value>) -> Value {
    let kind = schema["type"].as_array().and_then(|kinds| kinds.iter().find(|kind| *kind != "null")).map(|kind| json!({"type": kind}));
    let properties = schema["properties"].as_object().map(|properties| json!({"properties": properties.iter().map(|(name, property)| (name.clone(), resolved(property, refs))).collect::<Map<_, _>>()}));
    let typed = merged(schema.clone(), kind.unwrap_or_default());
    match (properties, typed["type"].is_null()) {
        (Some(properties), true) => merged(merged(typed, properties), json!({"type": "object"})),
        (Some(properties), false) => merged(typed, properties),
        (None, _) => typed,
    }
}

fn joined(base: Value, part: Value) -> Value {
    let properties = merged(base["properties"].clone(), part["properties"].clone());
    let whole = merged(base, part);
    merged(whole, json!({"properties": properties}))
}

fn bound(key: &str) -> Value {
    json!({"@path": format!("/{key}")})
}

fn merged(base: Value, extra: Value) -> Value {
    Value::Object(base.as_object().into_iter().chain(extra.as_object()).flatten().map(|(k, v)| (k.clone(), v.clone())).collect())
}

fn options(pairs: &[(&str, &str)]) -> Vec<Value> {
    pairs.iter().map(|(label, value)| json!({"label": label, "value": value})).collect()
}

fn open_url(url: &str) -> Value {
    json!({"functionCall": {"@call": "openUrl", "args": {"url": url}}})
}

fn shown(value: &Value) -> String {
    value.as_str().map_or_else(|| value.to_string(), str::to_string)
}

/// Every button's event carries the inputs of the surface, each top-level key bound, so the action holds what was filled.
fn answering(components: Vec<Value>) -> Vec<Value> {
    let keys = components.iter().filter_map(|c| c["value"]["@path"].as_str()?.split('/').nth(1).map(str::to_string)).unique();
    let context = Value::Object(keys.map(|key| (key.clone(), bound(&key))).collect());
    components.into_iter().map(|component| with_context(component, &context)).collect()
}

fn with_context(mut component: Value, context: &Value) -> Value {
    component.pointer_mut("/action/event").and_then(Value::as_object_mut).into_iter().for_each(|event| {
        event.insert("context".to_string(), context.clone());
    });
    component
}

/// What surrounds a surface when it is sent: the sender's avatar, the time shown, and the read mark's icons
/// (sent, read), kept in the data model so a read can change it.
#[derive(Default)]
pub(super) struct Frame {
    pub avatar: Option<Value>,
    pub time: Option<String>,
    pub mark: Option<(Value, Value)>,
}

impl Frame {
    fn footer(&self) -> Vec<Value> {
        let time = self.time.iter().map(|text| json!({"id": "time", "component": "Text", "variant": "caption", "text": text}));
        let mark = self.mark.iter().map(|_| json!({"id": "mark", "component": "Icon", "name": bound("meta/mark")}));
        time.chain(mark).collect()
    }

    fn wrap(&self, top: &[String]) -> Vec<Value> {
        let footer = self.footer();
        let ids = footer.iter().map(|part| part["id"].clone()).collect::<Vec<_>>();
        let row = (!ids.is_empty()).then(|| json!({"id": "footer", "component": "Row", "children": ids}));
        let body = top.iter().map(|id| json!(id)).chain(row.iter().map(|_| json!("footer"))).collect::<Vec<_>>();
        let outer = self.avatar.iter().map(|_| json!("avatar")).chain([json!("body")]).collect::<Vec<_>>();
        let frame = [json!({"id": "root", "component": "Row", "children": outer}), json!({"id": "body", "component": "Column", "children": body})];
        frame.into_iter().chain(self.avatar.clone()).chain(row).chain(footer).collect()
    }

    /// The data model: the surface's data, and the read mark's icons under `meta`.
    fn model(&self, data: &Value) -> Value {
        let meta = self.mark.as_ref().map_or(json!({}), |(sent, read)| json!({"meta": {"mark": sent, "sent": sent, "read": read}}));
        let model = merged(data.clone(), meta);
        model.as_object().filter(|fields| !fields.is_empty()).map_or(json!({}), |_| json!({"dataModel": model}))
    }

    fn is_empty(&self) -> bool {
        self.avatar.is_none() && self.time.is_none() && self.mark.is_none()
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Surface {
    prefix: String,
    components: Vec<Value>,
    top: Vec<String>,
    data: Value,
    labels: Vec<String>,
}

impl Surface {
    pub fn new() -> Self {
        Self::default()
    }

    /// The a2ui 1.0 messages that create the surface `surface_id` and fill it.
    pub fn messages(&self, surface_id: &str) -> Vec<Value> {
        self.framed(surface_id, &Frame::default())
    }

    pub(super) fn framed(&self, surface_id: &str, frame: &Frame) -> Vec<Value> {
        let (top, labels) = self.labelled();
        let outer = match frame.is_empty() {
            true => vec![json!({"id": "root", "component": "Column", "children": top})],
            false => frame.wrap(&top),
        };
        let components = answering(outer.into_iter().chain(labels).chain(self.components.clone()).collect());
        let create = merged(json!({"surfaceId": surface_id, "catalogId": CATALOG, "sendDataModel": true}), frame.model(&self.data));
        vec![
            json!({"version": VERSION, "createSurface": create}),
            json!({"version": VERSION, "updateComponents": {"surfaceId": surface_id, "components": components}}),
        ]
    }

    /// The top-level ids with the labels' row first, and the labels' components.
    fn labelled(&self) -> (Vec<String>, Vec<Value>) {
        let ids = (0..self.labels.len()).map(|n| format!("label{n}")).collect::<Vec<_>>();
        let texts = self.labels.iter().zip(&ids).map(|(text, id)| json!({"id": id, "component": "Text", "variant": "caption", "text": text}));
        let row = (!ids.is_empty()).then(|| json!({"id": "labels", "component": "Row", "children": ids}));
        let top = row.iter().map(|_| "labels".to_string()).chain(self.top.clone()).collect();
        (top, row.into_iter().chain(texts).collect())
    }

    fn within(prefix: String) -> Self {
        Self { prefix, ..Self::default() }
    }

    fn next_id(&self) -> String {
        format!("{}c{}", self.prefix, self.components.len())
    }

    fn push(self, id: String, parts: Vec<Value>) -> Self {
        Self { components: [self.components, parts].concat(), top: [self.top, vec![id]].concat(), ..self }
    }

    fn leaf(self, component: &str, props: Value) -> Self {
        let id = self.next_id();
        let node = merged(json!({"id": id, "component": component}), props);
        self.push(id, vec![node])
    }

    fn nest(self, build: impl FnOnce(Surface) -> Surface, shape: impl FnOnce(&str, Vec<String>) -> Vec<Value>) -> Self {
        let id = self.next_id();
        let inner = build(Surface::within(format!("{id}_")));
        let parts = shape(&id, inner.top).into_iter().chain(inner.components).collect();
        let data = merged(self.data.clone(), inner.data);
        Self { data, ..self.push(id, parts) }
    }

    fn pressable(self, title: &str, action: Value) -> Self {
        let id = self.next_id();
        let button = json!({"id": id, "component": "Button", "child": format!("{id}t"), "action": action});
        let label = json!({"id": format!("{id}t"), "component": "Text", "text": title});
        self.push(id, vec![button, label])
    }

    pub fn text(self, text: &str) -> Self {
        self.leaf("Text", json!({"text": text}))
    }

    pub fn title(self, text: &str) -> Self {
        self.leaf("Text", json!({"text": format!("**{text}**")}))
    }

    pub fn subtitle(self, text: &str) -> Self {
        self.leaf("Text", json!({"text": text, "variant": "caption"}))
    }

    /// One line: the name and its value.
    pub fn field(self, name: &str, value: &str) -> Self {
        let id = self.next_id();
        let row = json!({"id": id, "component": "Row", "children": [format!("{id}n"), format!("{id}v")]});
        let label = json!({"id": format!("{id}n"), "component": "Text", "variant": "caption", "text": name});
        let shown = json!({"id": format!("{id}v"), "component": "Text", "text": value});
        self.push(id, vec![row, label, shown])
    }

    /// One field per property of `value`, in its order; a property with no value is left out.
    pub fn fields(self, value: &impl Serialize) -> Self {
        let value = serde_json::to_value(value).unwrap_or_default();
        let present = value.as_object().into_iter().flatten().filter(|(_, value)| !value.is_null());
        present.fold(self, |surface, (name, value)| surface.field(name, &shown(value)))
    }

    /// One input per property of the schema, in its order, bound to the data model under the property's name.
    pub fn inputs(self, schema: Schema) -> Self {
        self.inputs_under("", &schema.0)
    }

    fn inputs_under(self, path: &str, schema: &Value) -> Self {
        let properties = schema["properties"].as_object().into_iter().flatten();
        properties.fold(self, |surface, (name, property)| surface.input_for(&format!("{path}{name}"), name, property))
    }

    fn input_for(self, key: &str, label: &str, property: &Value) -> Self {
        match (property["type"].as_str(), property["format"].as_str(), property["enum"].as_array()) {
            (_, _, Some(choices)) => self.choice(key, &choices.iter().filter_map(Value::as_str).map(|choice| (choice, choice)).collect::<Vec<_>>()),
            (Some("string"), Some("date" | "date-time"), _) => self.calendar(key),
            (Some("integer" | "number"), _, _) => self.number(label, key),
            (Some("boolean"), _, _) => self.leaf("CheckBox", json!({"label": label, "value": bound(key)})),
            (Some("object"), _, _) => self.column(|column| column.inputs_under(&format!("{key}/"), property)),
            _ => self.input(label, key),
        }
    }

    /// The data model the inputs show, such as a value to correct.
    pub fn filled(self, data: &impl Serialize) -> Self {
        let data = merged(self.data.clone(), serde_json::to_value(data).unwrap_or_default());
        Self { data, ..self }
    }

    /// Strings shown with the message.
    pub fn labels(self, labels: &[&str]) -> Self {
        Self { labels: labels.iter().map(|label| label.to_string()).collect(), ..self }
    }

    /// A button; `event` is the name of the action sent back when it is pressed.
    pub fn button(self, title: &str, event: &str) -> Self {
        self.pressable(title, json!({"event": {"name": event, "userMessage": title}}))
    }

    pub fn row(self, build: impl FnOnce(Surface) -> Surface) -> Self {
        self.nest(build, |id, kids| vec![json!({"id": id, "component": "Row", "children": kids})])
    }

    pub fn column(self, build: impl FnOnce(Surface) -> Surface) -> Self {
        self.nest(build, |id, kids| vec![json!({"id": id, "component": "Column", "children": kids})])
    }

    pub fn card(self, build: impl FnOnce(Surface) -> Surface) -> Self {
        self.nest(build, |id, kids| {
            vec![
                json!({"id": id, "component": "Card", "child": format!("{id}b")}),
                json!({"id": format!("{id}b"), "component": "Column", "children": kids}),
            ]
        })
    }

    /// Cards side by side in a horizontal list.
    pub fn carousel(self, build: impl FnOnce(Surface) -> Surface) -> Self {
        self.nest(build, |id, kids| vec![json!({"id": id, "component": "List", "direction": "horizontal", "children": kids})])
    }

    /// A text field whose value is kept in the data model under `key`.
    pub fn input(self, label: &str, key: &str) -> Self {
        self.leaf("TextField", json!({"label": label, "value": bound(key)}))
    }

    pub fn number(self, label: &str, key: &str) -> Self {
        self.leaf("TextField", json!({"label": label, "value": bound(key), "variant": "number"}))
    }

    pub fn choice(self, key: &str, choices: &[(&str, &str)]) -> Self {
        self.leaf("ChoicePicker", json!({"options": options(choices), "value": bound(key), "variant": "mutuallyExclusive"}))
    }

    pub fn calendar(self, key: &str) -> Self {
        self.leaf("DateTimeInput", json!({"value": bound(key), "enableDate": true}))
    }

    /// The fields, the question, and an approve and a reject button.
    pub fn approval(question: &str, fields: &[(&str, &str)]) -> Self {
        let shown = fields.iter().fold(Self::new(), |surface, (name, value)| surface.field(name, value));
        shown.text(question).row(|row| row.button("Approve", "approve").button("Reject", "reject"))
    }

    pub fn selection(prompt: &str, choices: &[(&str, &str)], counted: bool) -> Self {
        let picked = Self::new().text(prompt).choice("selected", choices);
        match counted {
            true => picked.number("Count", "count").button("Send", "submit"),
            false => picked.button("Send", "submit"),
        }
    }

    pub fn login(url: &str) -> Self {
        Self::new().pressable("Log in", open_url(url))
    }

    pub fn logout(url: &str) -> Self {
        Self::new().pressable("Log out", open_url(url))
    }

    pub fn times(prompt: &str, slots: &[(&str, &str)]) -> Self {
        Self::new().text(prompt).choice("slot", slots).button("Send", "submit")
    }

    pub fn date(prompt: &str) -> Self {
        Self::new().text(prompt).calendar("from").button("Send", "submit")
    }

    pub fn range(prompt: &str) -> Self {
        Self::new().text(prompt).calendar("from").calendar("to").button("Send", "submit")
    }

    /// The client's own schema as a form, without what the server sets.
    #[cfg(feature = "axum")]
    pub fn intake() -> Self {
        let client = Schema::of::<crate::booking::client::ClientData>().without(&["tenant_id", "attributes"]);
        Self::new().inputs(client).button("Send", "submit")
    }

    pub fn feedback(scale: Scale) -> Self {
        let rated = match scale {
            Scale::Stars(stars) => Self::new().leaf("Slider", json!({"label": "Rating", "min": 1, "max": stars, "value": bound("rating")})),
            Scale::Words(words) => Self::new().choice("rating", &words.iter().map(|word| (*word, *word)).collect::<Vec<_>>()),
        };
        rated.input("Comment", "comment").button("Send", "submit")
    }
}
