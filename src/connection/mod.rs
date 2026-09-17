//! Connection domain for external services and OAuth authorizations.

pub mod model;
#[cfg(feature = "postgres")]
pub mod repo;

pub use model::{
    Connection, ConnectionData, ConnectionExt, ConnectionView, FullConnection, OAuth2Tokens,
    UpsertConnectionInput,
};
#[cfg(feature = "postgres")]
pub use repo::ConnectionRepo;
