//! Generic booking: services, bookables, schedules and blockouts, appointments. See `docs/TPD-booking.md`.

mod appointments;
#[cfg(feature = "postgres")]
mod calendar;
#[cfg(feature = "postgres")]
mod groups;
mod model;
#[cfg(feature = "postgres")]
mod postgres;
mod recurrence;
#[cfg(feature = "postgres")]
mod rules;
mod schedule;
mod templates;
mod times;

pub use appointments::*;
pub use model::*;
#[cfg(feature = "postgres")]
pub use postgres::Calendar;
#[cfg(feature = "postgres")]
use groups::Groups;
pub use schedule::*;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    #[error("not open: {0:?}")]
    NotOpen(Axes),
    #[error("occupied: {0:?}")]
    Occupied(BookableId),
    #[error("client busy: {0:?}")]
    ClientBusy(ClientId),
    #[error("hold expired: {0:?}")]
    HoldExpired(HoldId),
    #[error("not found")]
    NotFound,
    #[error("idempotency key reused with a different request")]
    KeyReused,
    #[error("grouping cycle")]
    Cycle,
    #[error("recurrence: {0}")]
    Recurrence(String),
    #[error("insufficient balance on {0:?}")]
    Insufficient(PaymentMethodId),
    #[cfg(feature = "postgres")]
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}
