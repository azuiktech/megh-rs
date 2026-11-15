//! a2ui v1.0 validation against the vendored schemas.

use std::sync::LazyLock;

use jsonschema::{Registry, Resource, Validator};
use serde_json::Value;

use crate::messaging::MessagingError;

const BASE: &str = "https://a2ui.org/specification/v1_0";
const SCHEMAS: [(&str, &str); 4] = [
    ("agent_to_renderer.json", include_str!("../spec/agent_to_renderer.json")),
    ("common_types.json", include_str!("../spec/common_types.json")),
    ("catalogs/basic/common_types.json", include_str!("../spec/common_types.json")),
    ("catalog.json", include_str!("../spec/catalog.json")),
];

static MESSAGE: LazyLock<Validator> = LazyLock::new(|| {
    let registry = SCHEMAS
        .iter()
        .try_fold(Registry::new(), |registry, (path, schema)| {
            let schema = serde_json::from_str(schema).expect("a vendored schema is JSON");
            registry.add(format!("{BASE}/{path}"), Resource::from_contents(schema))
        })
        .and_then(|registry| registry.prepare())
        .expect("the vendored schemas resolve");
    jsonschema::options()
        .with_registry(&registry)
        .build(&serde_json::json!({"$ref": format!("{BASE}/agent_to_renderer.json")}))
        .expect("the vendored message schema compiles")
});

pub(super) fn check(messages: &[Value]) -> Result<(), MessagingError> {
    messages.iter().enumerate().try_for_each(|(n, message)| {
        MESSAGE.validate(message).map_err(|error| {
            MessagingError::InvalidSurface(format!("message {n}: {error} (at '{}')", error.instance_path()))
        })
    })
}
