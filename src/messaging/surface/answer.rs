//! The typed answers to the ready-made surfaces: the `context` of a `submit` action, refused when it is another surface's.

use chrono::{DateTime, FixedOffset, NaiveDate};
use rust_decimal::Decimal;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::Action;
use crate::booking::value::{Contact, Name};
use crate::messaging::MessagingError;

fn invalid(error: serde_json::Error) -> MessagingError {
    MessagingError::InvalidSurface(format!("answer: {error}"))
}

pub trait FromAction: DeserializeOwned {
    fn from_action(action: &Action) -> Result<Self, MessagingError> {
        serde_json::from_value(action.context.clone()).map_err(invalid)
    }
}

/// The `answer` of an answer message, read as `T`.
pub fn answer<T: FromAction>(message: &impl Serialize) -> Result<T, MessagingError> {
    let message = serde_json::to_value(message).map_err(invalid)?;
    let action: Action = serde_json::from_value(message["answer"].clone()).map_err(invalid)?;
    T::from_action(&action)
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Selected {
    pub selected: Vec<Pick>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Pick {
    pub id: Uuid,
    #[serde(default)]
    pub count: Option<Decimal>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Dates {
    pub from: NaiveDate,
    #[serde(default)]
    pub to: Option<NaiveDate>,
}

/// The fields of a client, as the intake surface asks for them.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Intake {
    pub first_name: Name,
    pub last_name: Name,
    pub name: Name,
    #[serde(flatten)]
    pub contact: Contact,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rated {
    #[serde(default)]
    pub rating: Option<u32>,
    #[serde(default)]
    pub choice: Option<String>,
    #[serde(default)]
    pub comment: Option<String>,
    #[serde(default)]
    pub attachment: Option<Uuid>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Verdict {
    Approved,
    Rejected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Decision {
    pub decision: Verdict,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Slot {
    pub starts_at: DateTime<FixedOffset>,
    pub ends_at: DateTime<FixedOffset>,
    pub service_id: Uuid,
}

/// What was picked from open times, quick replies or buttons.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Picked {
    Slot(Slot),
    Reply(String),
    Button(String),
}

impl FromAction for Selected {}
impl FromAction for Dates {}
impl FromAction for Intake {}
impl FromAction for Rated {}
impl FromAction for Decision {}
impl FromAction for Picked {}
