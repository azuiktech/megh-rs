//! How a booking error is told to a client (Google AIP-193).

use axum::http::StatusCode;

pub use crate::auth::api::ApiError;
use crate::booking::Error;

impl From<Error> for ApiError {
    fn from(error: Error) -> Self {
        let message = error.to_string();
        match error {
            Error::NotFound => Self::new(StatusCode::NOT_FOUND, "NOT_FOUND", message),
            Error::Invalid(_) | Error::Recurrence(_) => Self::invalid(message),
            Error::NotOpen(_) | Error::HoldExpired(_) | Error::Insufficient(_) | Error::Expired(_) | Error::NotEligible(_) | Error::Cycle => Self::new(StatusCode::BAD_REQUEST, "FAILED_PRECONDITION", message),
            Error::Occupied(_) | Error::ClientBusy(_) => Self::new(StatusCode::CONFLICT, "ABORTED", message),
            Error::KeyReused => Self::conflict(message),
            Error::Database(error) => Self::internal(error),
        }
    }
}
