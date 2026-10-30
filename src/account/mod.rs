//! Connected accounts domain matching Go megh connected_accounts table.

pub mod model;
#[cfg(feature = "postgres")]
pub mod encryption;
#[cfg(feature = "postgres")]
pub mod repo;
#[cfg(all(feature = "postgres", feature = "client"))]
pub mod accounts;
#[cfg(all(feature = "postgres", feature = "client"))]
pub mod connections;
#[cfg(all(feature = "postgres", feature = "client"))]
mod site_oauth;

pub use model::{ConnectedAccount, OAuth2Tokens};
#[cfg(feature = "postgres")]
pub use encryption::{EncryptError, Encryptor};
#[cfg(feature = "postgres")]
pub use repo::ConnectedAccountRepo;
#[cfg(all(feature = "postgres", feature = "client"))]
pub use accounts::{AccountAuth, AccountError, Accounts};
#[cfg(all(feature = "postgres", feature = "client"))]
pub use connections::{Connection, Connections, Secret};
#[cfg(all(feature = "axum", feature = "postgres", feature = "client"))]
pub use connections::connections_router;
