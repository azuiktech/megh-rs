//! Connected accounts domain matching Go megh connected_accounts table.

pub mod model;
#[cfg(feature = "sql")]
pub mod encryption;
#[cfg(feature = "sql")]
pub mod repo;
#[cfg(all(feature = "sql", feature = "client"))]
pub mod accounts;
#[cfg(all(feature = "sql", feature = "client"))]
pub mod connections;
#[cfg(all(feature = "sql", feature = "client"))]
mod site_oauth;

pub use model::{ConnectedAccount, OAuth2Tokens};
#[cfg(feature = "sql")]
pub use encryption::{EncryptError, Encryptor};
#[cfg(feature = "sql")]
pub use repo::ConnectedAccountRepo;
#[cfg(all(feature = "sql", feature = "client"))]
pub use accounts::{AccountAuth, AccountError, Accounts};
#[cfg(all(feature = "sql", feature = "client"))]
pub use connections::{Connection, Connections, Secret};
#[cfg(all(feature = "axum", feature = "sql", feature = "client"))]
pub use connections::connections_router;
