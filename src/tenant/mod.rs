//! Tenancy and membership domain.

#[cfg(feature = "postgres")]
pub mod lifecycle;
pub mod member;
pub mod model;
#[cfg(feature = "postgres")]
pub mod tenants;

#[cfg(feature = "postgres")]
pub use lifecycle::{Change, Lifecycle, LifecycleError};
pub use member::Member;
pub use model::Tenant;
#[cfg(feature = "postgres")]
pub use tenants::{TenantError, Tenants};
