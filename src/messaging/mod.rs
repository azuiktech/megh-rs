//! In-app messaging between a client and a member of one tenant: canned a2ui 1.0 surfaces carried over the broker.

mod attachment;
mod identity;
mod message;
mod surface;

pub use attachment::{File, FileData};

pub(crate) use identity::{CLIENT, current};
pub use identity::{follows, guests};
pub use message::{Avatar, History, Icon, Message, Messaging, MessagingError, Notice, Party, ReadMark, Signal, TimeFormat, router};
pub use surface::{Scale, Schema, Surface};
