//! Event data models and subscription payloads.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use super::topic::Topic;

/// Identifies where an event originated during transmission.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventSource {
    /// Real-time event broadcast as it happens.
    Live,
    /// Transient event replayed from the in-memory ring buffer after a network glitch.
    Buffered,
    /// Long-term historical event fetched from a persistent HistoryProvider.
    History,
}

/// An application-level event published to a topic.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Event {
    pub id: String,
    pub topic: Topic,
    pub data: serde_json::Value,
    pub timestamp: DateTime<Utc>,
}

impl Event {
    /// Creates a new event with current UTC timestamp.
    pub fn new(topic: impl Into<Topic>, data: serde_json::Value) -> Self {
        Self {
            id: String::new(),
            topic: topic.into(),
            data,
            timestamp: Utc::now(),
        }
    }

    /// Sets the event ID.
    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = id.into();
        self
    }
}

/// Filtering parameters for history hydration.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct HistoryOptions {
    #[serde(default)]
    pub since: Option<DateTime<Utc>>,
    #[serde(default)]
    pub limit: Option<usize>,
}

/// Payload for updating client subscriptions.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SubscriptionRequest {
    pub client_id: String,
    #[serde(default)]
    pub subscribe: Vec<Topic>,
    #[serde(default)]
    pub unsubscribe: Vec<Topic>,
    #[serde(default)]
    pub last_event_id: Option<String>,
}

/// Response returned after updating subscriptions.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SubscriptionResponse {
    pub client_id: String,
    pub subscriptions: Vec<Topic>,
}
