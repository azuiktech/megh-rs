//! Connected accounts domain matching Go megh connected_accounts table.

pub mod model;
#[cfg(feature = "postgres")]
pub mod repo;
#[cfg(all(feature = "postgres", feature = "client"))]
pub mod accounts;

pub use model::{ConnectedAccount, OAuth2Tokens};
#[cfg(feature = "postgres")]
pub use repo::ConnectedAccountRepo;
#[cfg(all(feature = "postgres", feature = "client"))]
pub use accounts::{AccountAuth, AccountError, Accounts};
