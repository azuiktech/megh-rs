//! The Postgres-backed aggregates. Each holds the injected pool, as `Orgs` does.

use sqlx::PgPool;

/// Open time of every combination: implements `Availability`.
#[derive(Clone)]
pub struct Calendar {
    pub(super) pool: PgPool,
}

/// Slots that satisfy a search: implements `Search`.
#[derive(Clone)]
pub struct Slots {
    pub(super) pool: PgPool,
}

impl Slots {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

impl Calendar {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}
