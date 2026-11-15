//! The handle of messaging: the pool, the broker events go out on, and the attachment limit.

use serde::Deserialize;
use sqlx::PgPool;
use uuid::Uuid;
use validator::Validate;

use crate::auth::rest::PageRequest;
use crate::event::{Broker, Event};

const ATTACHMENT_LIMIT: usize = 10 * 1024 * 1024;

#[derive(Clone)]
pub struct Messaging {
    pub(super) pool: PgPool,
    broker: Broker,
    attachment_limit: usize,
}

impl Messaging {
    pub fn new(pool: PgPool, broker: Broker) -> Self {
        Self { pool, broker, attachment_limit: ATTACHMENT_LIMIT }
    }

    /// The most bytes an attachment may hold; 10 MiB unless set here.
    pub fn with_attachment_limit(self, bytes: usize) -> Self {
        Self { attachment_limit: bytes, ..self }
    }

    pub fn attachment_limit(&self) -> usize {
        self.attachment_limit
    }

    pub(super) async fn announce(&self, event: Event) {
        self.broker.publish(event).await;
    }
}

pub(super) fn thread_topic(tenant_id: Uuid, thread_id: Uuid) -> String {
    format!("tenants/{tenant_id}/threads/{thread_id}")
}

pub(super) fn threads_topic(tenant_id: Uuid) -> String {
    format!("tenants/{tenant_id}/threads")
}

/// A page of what a thread holds: the tenant and thread of the path, and the page asked for.
#[derive(Debug, Deserialize, Validate)]
pub(super) struct Paged {
    pub tenant_id: Uuid,
    pub id: Uuid,
    #[serde(flatten)]
    pub page: PageRequest,
}
