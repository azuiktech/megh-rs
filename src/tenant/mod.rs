//! Tenancy and membership domain.

#[cfg(feature = "sql")]
pub mod lifecycle;
pub mod member;
pub mod model;
#[cfg(feature = "sql")]
pub mod tenants;

#[cfg(feature = "sql")]
pub use lifecycle::{Change, Lifecycle, LifecycleError};
pub use member::Member;
pub use model::Tenant;
#[cfg(feature = "sql")]
pub use tenants::{TenantError, Tenants};
