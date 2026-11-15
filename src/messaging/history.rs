//! Catch-up for a client that reconnects: the messages of its thread it missed, from the database.

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use super::message::{message_event, select};
use crate::event::{Event, HistoryError, HistoryFuture, HistoryOptions, HistoryProvider, Topic};

/// The `HistoryProvider` of the topics `tenants/{t}/threads/{id}`; every other topic has no history.
#[derive(Clone)]
pub struct History {
    pool: PgPool,
}

impl History {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    async fn events(&self, topic: &Topic, after: Option<Uuid>, since: Option<DateTime<Utc>>, limit: Option<usize>) -> Result<Vec<Event>, HistoryError> {
        let Some((tenant_id, thread_id)) = thread_of(topic) else { return Ok(Vec::new()) };
        let mut query = select(tenant_id);
        query
            .push(" AND m.thread_id = ")
            .push_bind(thread_id)
            .push(" AND (")
            .push_bind(after)
            .push("::uuid IS NULL OR m.id > ")
            .push_bind(after)
            .push(") AND (")
            .push_bind(since)
            .push("::timestamptz IS NULL OR m.created_at > ")
            .push_bind(since)
            .push(") ORDER BY m.id LIMIT ")
            .push_bind(limit.map(|limit| limit as i64));
        let messages: Vec<super::Message> = query.build_query_as().fetch_all(&self.pool).await?;
        Ok(messages.iter().map(|message| Event { timestamp: message.created_at, ..message_event(message) }).collect())
    }
}

fn thread_of(topic: &Topic) -> Option<(Uuid, Uuid)> {
    match topic.split('/').collect::<Vec<_>>()[..] {
        ["tenants", tenant, "threads", thread] => Some((tenant.parse().ok()?, thread.parse().ok()?)),
        _ => None,
    }
}

impl HistoryProvider for History {
    fn fetch_history<'a>(&'a self, topic: &'a Topic, opts: &'a HistoryOptions) -> HistoryFuture<'a> {
        Box::pin(self.events(topic, None, opts.since, opts.limit))
    }

    fn fetch_history_since_id<'a>(&'a self, topic: &'a Topic, last_id: &'a str, limit: Option<usize>) -> HistoryFuture<'a> {
        Box::pin(async move {
            match last_id.parse() {
                Ok(after) => self.events(topic, Some(after), None, limit).await,
                Err(_) => Ok(Vec::new()),
            }
        })
    }
}
