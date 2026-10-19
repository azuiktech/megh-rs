//! Generic booking: services, bookables, schedules and blockouts, appointments. See `docs/TPD-booking.md`.

mod appointments;
#[cfg(feature = "postgres")]
mod busy;
#[cfg(feature = "postgres")]
mod calendar;
#[cfg(feature = "postgres")]
mod groups;
#[cfg(all(feature = "axum", feature = "postgres"))]
mod http;
#[cfg(feature = "postgres")]
mod market;
mod filter;
mod model;
#[cfg(feature = "postgres")]
mod postgres;
#[cfg(feature = "postgres")]
mod recurrence;
#[cfg(feature = "postgres")]
mod reservations;
#[cfg(feature = "postgres")]
mod rules;
mod schedule;
#[cfg(feature = "postgres")]
mod search;
mod templates;
mod times;
mod variants;

pub use appointments::*;
pub use filter::Filter;
#[cfg(all(feature = "axum", feature = "postgres"))]
pub use http::{booking_router, booking_router_with_csrf, openapi};
pub use model::*;
#[cfg(feature = "postgres")]
pub use postgres::{Calendar, Reservations, Slots};
#[cfg(feature = "postgres")]
pub use reservations::booking_print;
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
    #[error("an appointment cannot go from {0:?} to {1:?}")]
    Transition(Status, Status),
    #[error("hold expired: {0:?}")]
    HoldExpired(AppointmentId),
    #[error("not found")]
    NotFound,
    #[error("idempotency key reused with a different request")]
    KeyReused,
    #[error("grouping cycle")]
    Cycle,
    #[error("invalid: {0}")]
    Invalid(String),
    #[error("recurrence: {0}")]
    Recurrence(String),
    #[error("insufficient balance on {0:?}")]
    Insufficient(PaymentMethodId),
    #[error("{0:?} is not valid at the time of the appointment")]
    Expired(PaymentMethodId),
    #[error("{0:?} does not cover the services booked")]
    NotEligible(PaymentMethodId),
    #[cfg(feature = "postgres")]
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}
