//! In-app messaging between a client and a member of one tenant: canned a2ui 1.0 surfaces carried over the broker.

#[cfg(all(feature = "axum", feature = "postgres"))]
mod message;
mod surface;

#[cfg(all(feature = "axum", feature = "postgres"))]
pub use message::{Avatar, History, Icon, Message, Messaging, MessagingError, Notice, Party, ReadMark, Signal, TimeFormat, router};
pub use surface::{Scale, Surface};
