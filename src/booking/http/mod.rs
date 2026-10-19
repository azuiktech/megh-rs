//! REST for the booking module, following the Google API Improvement Proposals: resources under
//! `/api/v1/orgs/{org_id}/...`, the standard methods Get, List, Create, Update and Delete (AIP-131 to 135), custom
//! methods as `:verb` (AIP-136), lists with `filter`, `page_size` and `page_token` (AIP-132, 158, 160), soft delete
//! with `show_deleted` (AIP-164), field names of AIP-148, durations as `"3600s"` (AIP-142) and errors of AIP-193.
//!
//! The router assumes the application authenticates the caller and applies [`crate::authorizer`], which derives
//! the grants from the paths: `orgs:read:{org_id}` and then the resource and action of the route.

mod availability;
mod bookables;
mod clients;
mod common;
mod crud;
mod error;
mod groupings;
mod page;
mod payment_methods;
mod schedules;
mod services;

use std::collections::HashMap;

use axum::Router;
use sqlx::PgPool;
use tower_http::csrf::CsrfLayer;
use utoipa::openapi::security::{ApiKey, ApiKeyValue, SecurityRequirement, SecurityScheme};
use utoipa::{Modify, OpenApi};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;
use uuid::Uuid;


use super::{Groups, Grouped, Kind, Result};

#[derive(Clone)]
pub struct AppContext {
    pub(super) pool: PgPool,
}

/// The booking routes. Writes are CSRF-protected by default (no trusted cross-origin callers); use
/// [`booking_router_with_csrf`] to configure trusted origins.
pub fn booking_router(pool: PgPool) -> Router {
    booking_router_with_csrf(pool, CsrfLayer::new())
}

/// The OpenAPI document of the routes. Only the routes registered with `routes!` are in it so far.
pub fn openapi() -> utoipa::openapi::OpenApi {
    routes().split_for_parts().1
}

pub fn booking_router_with_csrf(pool: PgPool, csrf: CsrfLayer) -> Router {
    routes().split_for_parts().0.layer(crate::auth::api::NegotiateLayer).layer(csrf).layer(axum::middleware::map_response(crate::auth::api::aip_193)).with_state(AppContext { pool })
}

#[derive(OpenApi)]
#[openapi(info(title = "Megh booking API", version = "1"), modifiers(&Authentication), tags((name = "bookables", description = "Providers and rooms."), (name = "clients", description = "People, pets and others who are served."), (name = "groupings", description = "The hierarchy of orgs, bookables, services and clients.")))]
struct Booking;

/// The cookie `jwt_token` that `jwt_session` issues from the session after sign-in.
struct Authentication;

impl Modify for Authentication {
    fn modify(&self, api: &mut utoipa::openapi::OpenApi) {
        let cookie = SecurityScheme::ApiKey(ApiKey::Cookie(ApiKeyValue::new("jwt_token")));
        api.components.get_or_insert_with(Default::default).add_security_scheme("jwtCookie", cookie);
        api.security = Some(vec![SecurityRequirement::new("jwtCookie", Vec::<String>::new())]);
    }
}

fn routes() -> OpenApiRouter<AppContext> {
    OpenApiRouter::with_openapi(Booking::openapi())
        .routes(routes!(bookables::list, bookables::create))
        .routes(routes!(bookables::read, bookables::replace, bookables::patch, bookables::remove))
        .routes(routes!(clients::list, clients::create))
        .routes(routes!(clients::read, clients::replace, clients::patch, clients::remove))
        .routes(routes!(groupings::list, groupings::create))
        .routes(routes!(groupings::read, groupings::remove))
        .merge(OpenApiRouter::from(others()))
}

fn others() -> Router<AppContext> {
    Router::new()
        .merge(crud::routes::<services::Services>())
        .merge(crud::routes::<payment_methods::PaymentMethods>())
        .merge(availability::routes())
        .merge(schedules::routes("schedules", Kind::Open))
        .merge(schedules::routes("blockouts", Kind::Blockout))
}

/// The org and every org grouped under it, at any depth.
pub(super) async fn subtree(pool: &PgPool, root: Uuid) -> Result<Vec<Uuid>> {
    let groups = Groups::load(pool, Grouped::Org).await?;
    let orgs: Vec<Uuid> = sqlx::query_scalar("SELECT id FROM organizations").fetch_all(pool).await?;
    let reaching: Vec<(Uuid, HashMap<Uuid, usize>)> = orgs.into_iter().map(|org| Ok((org, groups.reach(org)?))).collect::<Result<_>>()?;
    Ok(reaching.into_iter().filter(|(_, reach)| reach.contains_key(&root)).map(|(org, _)| org).collect())
}
