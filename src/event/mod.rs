//! Realtime event bus, topic routing, and transient SSE replay.

pub mod broker;
pub mod history;
pub mod model;
pub mod ring;
pub mod topic;

#[cfg(feature = "axum")]
pub mod sse;

pub use broker::Broker;
pub use history::{HistoryError, HistoryFuture, HistoryProvider, NoopHistoryProvider};
pub use model::{Event, EventSource, HistoryOptions, SubscriptionRequest, SubscriptionResponse};
pub use ring::RingBuffer;
pub use topic::Topic;

#[cfg(feature = "axum")]
pub use sse::events_router;
