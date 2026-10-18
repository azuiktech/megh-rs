//! REST for the booking module, following the Google API Improvement Proposals: resources under
//! `/api/v1/org/{org_id}/...`, the standard methods Get, List, Create, Update and Delete (AIP-131 to 135), custom
//! methods as `:verb` (AIP-136), lists with `filter`, `page_size` and `page_token` (AIP-132, 158, 160), soft delete
//! with `show_deleted` (AIP-164), field names of AIP-148, durations as `"3600s"` (AIP-142) and errors of AIP-193.
//!
//! The router assumes the application authenticates the caller and applies [`crate::authorizer`], which derives
//! the grants from the paths: `org:read:{org_id}` and then the resource and action of the route.

mod error;
mod page;
mod schedules;

use std::collections::HashMap;

use axum::Router;
use sqlx::PgPool;
use tower_http::csrf::CsrfLayer;
use uuid::Uuid;

pub use error::ApiError;

use super::{Groups, Grouped, Kind, Result};

#[derive(Clone)]
pub struct BookingState {
    pub(super) pool: PgPool,
}

/// The booking routes. Writes are CSRF-protected by default (no trusted cross-origin callers); use
/// [`booking_router_with_csrf`] to configure trusted origins.
pub fn booking_router(pool: PgPool) -> Router {
    booking_router_with_csrf(pool, CsrfLayer::new())
}

pub fn booking_router_with_csrf(pool: PgPool, csrf: CsrfLayer) -> Router {
    Router::new()
        .merge(schedules::routes("schedules", Kind::Open))
        .merge(schedules::routes("blockouts", Kind::Blockout))
        .layer(csrf)
        .with_state(BookingState { pool })
}

/// The org and every org grouped under it, at any depth.
pub(super) async fn subtree(pool: &PgPool, root: Uuid) -> Result<Vec<Uuid>> {
    let groups = Groups::load(pool, Grouped::Org).await?;
    let orgs: Vec<Uuid> = sqlx::query_scalar("SELECT id FROM organizations").fetch_all(pool).await?;
    let reaching: Vec<(Uuid, HashMap<Uuid, usize>)> = orgs.into_iter().map(|org| Ok((org, groups.reach(org)?))).collect::<Result<_>>()?;
    Ok(reaching.into_iter().filter(|(_, reach)| reach.contains_key(&root)).map(|(org, _)| org).collect())
}
