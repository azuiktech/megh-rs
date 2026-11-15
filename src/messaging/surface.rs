//! Surface: a fluent builder of a2ui 1.0 messages.

use serde_json::{Value, json};

const CATALOG: &str = "https://a2ui.org/specification/v1_0/catalogs/basic/catalog.json";

/// How feedback is given: a rating from 1 to the number of stars, or one of a few words.
pub enum Scale {
    Stars(u8),
    Words(&'static [&'static str]),
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Surface {
    prefix: String,
    components: Vec<Value>,
    top: Vec<String>,
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

    fn model(&self) -> Value {
        self.mark.as_ref().map_or(json!({}), |(sent, read)| json!({"dataModel": {"meta": {"mark": sent, "sent": sent, "read": read}}}))
    }

    fn is_empty(&self) -> bool {
        self.avatar.is_none() && self.time.is_none() && self.mark.is_none()
    }
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
        let outer = match frame.is_empty() {
            true => vec![json!({"id": "root", "component": "Column", "children": self.top})],
            false => frame.wrap(&self.top),
        };
        let components = outer.into_iter().chain(self.components.clone()).collect::<Vec<_>>();
        let create = merged(json!({"surfaceId": surface_id, "catalogId": CATALOG, "sendDataModel": true}), frame.model());
        vec![
            json!({"version": "1.0", "createSurface": create}),
            json!({"version": "1.0", "updateComponents": {"surfaceId": surface_id, "components": components}}),
        ]
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
        let inner = build(Surface::within(format!("{id}.")));
        let parts = shape(&id, inner.top).into_iter().chain(inner.components).collect();
        self.push(id, parts)
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

    /// One line: the name and its value.
    pub fn field(self, name: &str, value: &str) -> Self {
        let id = self.next_id();
        let row = json!({"id": id, "component": "Row", "children": [format!("{id}n"), format!("{id}v")]});
        let label = json!({"id": format!("{id}n"), "component": "Text", "variant": "caption", "text": name});
        let shown = json!({"id": format!("{id}v"), "component": "Text", "text": value});
        self.push(id, vec![row, label, shown])
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

    pub fn intake() -> Self {
        Self::new().input("Name", "name").input("Email", "email").input("Phone", "phone").calendar("birth_date").button("Send", "submit")
    }

    pub fn feedback(scale: Scale) -> Self {
        let rated = match scale {
            Scale::Stars(stars) => Self::new().leaf("Slider", json!({"label": "Rating", "min": 1, "max": stars, "value": bound("rating")})),
            Scale::Words(words) => Self::new().choice("rating", &words.iter().map(|word| (*word, *word)).collect::<Vec<_>>()),
        };
        rated.input("Comment", "comment").button("Send", "submit")
    }
}
