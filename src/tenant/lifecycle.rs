//! What changed in a tenant, and the app's function run after each change is committed.

use std::future::Future;
use std::pin::Pin;

use sqlx::PgPool;
use uuid::Uuid;

use super::{Member, Tenant};
use crate::billing::Subscription;

/// One committed change to a tenant, its members or its subscription.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum Change {
    TenantCreated(Tenant),
    TenantUpdated(Tenant),
    TenantRemoved(Uuid),
    MemberAdded(Member),
    MemberUpdated(Member),
    MemberRemoved(Member),
    Subscribed(Subscription),
    SubscriptionChanged(Subscription),
    SubscriptionCancelled(Subscription),
}

/// Why the app's function failed.
pub type LifecycleError = Box<dyn std::error::Error + Send + Sync>;

/// The app's function, run after each change with megh's pool. A plain `fn`, so what holds it stays `Clone`.
pub type Lifecycle = fn(PgPool, Change) -> Pin<Box<dyn Future<Output = Result<(), LifecycleError>> + Send>>;

/// Runs `lifecycle`, when the app gave one, on `change`.
pub(crate) async fn notify(lifecycle: Option<Lifecycle>, pool: &PgPool, change: Change) -> Result<(), LifecycleError> {
    match lifecycle {
        Some(lifecycle) => lifecycle(pool.clone(), change).await,
        None => Ok(()),
    }
}
