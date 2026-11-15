//! a2ui 1.0 surfaces: any valid one, the ready-made ones and the answers to them.

mod answer;
mod component;
mod form;
mod template;
mod validate;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::MessagingError;

pub use answer::{Dates, Decision, FromAction, Intake, Pick, Picked, Rated, Selected, Slot, Verdict, answer};
pub use form::{Choice, Prompt, Scale};
pub use template::{Button, Card, Reply};

/// A surface to send: its id and its a2ui 1.0 messages, already validated.
#[derive(Debug, Clone, PartialEq)]
pub struct Surface {
    pub id: String,
    pub messages: Vec<Value>,
}

impl Surface {
    /// Any a2ui 1.0 surface; its id is the `surfaceId` of its `createSurface` message.
    pub fn generic(messages: Vec<Value>) -> Result<Self, MessagingError> {
        validate::check(&messages)?;
        let id = messages
            .iter()
            .find_map(|message| message["createSurface"]["surfaceId"].as_str())
            .map(str::to_owned)
            .ok_or_else(|| MessagingError::InvalidSurface("no createSurface message".into()))?;
        Ok(Self { id, messages })
    }
}

/// What the user did on a surface: an a2ui renderer-to-agent action.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Action {
    pub name: String,
    #[serde(default)]
    pub context: Value,
    #[serde(default, rename = "userMessage", skip_serializing_if = "Option::is_none")]
    pub user_message: Option<String>,
}
