//! Connected accounts domain matching Go megh connected_accounts table.

pub mod model;
#[cfg(feature = "postgres")]
pub mod repo;

pub use model::{ConnectedAccount, OAuth2Tokens};
#[cfg(feature = "postgres")]
pub use repo::ConnectedAccountRepo;
