//! History provider interface and default implementations.

use super::model::{Event, HistoryOptions};
use super::topic::Topic;
use std::error::Error;
use std::future::Future;
use std::pin::Pin;

/// Type alias for history provider errors.
pub type HistoryError = Box<dyn Error + Send + Sync>;

/// Type alias for boxed future results in history providers.
pub type HistoryFuture<'a> =
    Pin<Box<dyn Future<Output = Result<Vec<Event>, HistoryError>> + Send + 'a>>;

/// Interface for persistent long-term event history backfill.
pub trait HistoryProvider: Send + Sync {
    /// Fetches historical events matching the topic and options.
    fn fetch_history<'a>(&'a self, topic: &'a Topic, opts: &'a HistoryOptions)
        -> HistoryFuture<'a>;

    /// Fetches historical events published strictly after `last_id`.
    fn fetch_history_since_id<'a>(
        &'a self,
        topic: &'a Topic,
        last_id: &'a str,
        limit: Option<usize>,
    ) -> HistoryFuture<'a>;
}

/// No-Op history provider returning empty results.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoopHistoryProvider;

impl HistoryProvider for NoopHistoryProvider {
    fn fetch_history<'a>(
        &'a self,
        _topic: &'a Topic,
        _opts: &'a HistoryOptions,
    ) -> HistoryFuture<'a> {
        Box::pin(async { Ok(Vec::new()) })
    }

    fn fetch_history_since_id<'a>(
        &'a self,
        _topic: &'a Topic,
        _last_id: &'a str,
        _limit: Option<usize>,
    ) -> HistoryFuture<'a> {
        Box::pin(async { Ok(Vec::new()) })
    }
}
