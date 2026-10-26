//! Tenancy and membership domain.

pub mod member;
pub mod model;
#[cfg(feature = "postgres")]
pub mod tenants;

pub use member::Member;
pub use model::Tenant;
#[cfg(feature = "postgres")]
pub use tenants::{TenantError, Tenants};
