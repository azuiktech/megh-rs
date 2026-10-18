//! Generic booking: services, bookables, schedules and blockouts, appointments. See `docs/TPD-booking.md`.

mod appointments;
mod model;
mod schedule;
mod templates;

pub use appointments::*;
pub use model::*;
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
