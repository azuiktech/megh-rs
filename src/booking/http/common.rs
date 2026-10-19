//! What the REST resources share: the orgs a request covers, and the scope of a search.

use chrono_tz::Tz;
use sqlx::PgPool;
use uuid::Uuid;

use super::subtree;
use crate::auth::api::ApiError;
use crate::booking::SearchRequest;

pub(super) async fn zone_of(pool: &PgPool, org: Uuid) -> Result<Tz, ApiError> {
    let found: Option<Option<String>> = sqlx::query_scalar("SELECT timezone FROM organizations WHERE id = $1").bind(org).fetch_optional(pool).await?;
    let name = found.ok_or_else(|| ApiError::not_found(format!("org {org} not found")))?;
    name.map_or(Ok(Tz::UTC), |name| name.parse().map_err(|_| ApiError::invalid(format!("the time zone {name} of the org is not known"))))
}

/// The orgs a request covers: the org of the path and those grouped under it.
pub(super) async fn scope(pool: &PgPool, org: Uuid) -> Result<Vec<Uuid>, ApiError> {
    zone_of(pool, org).await?;
    Ok(subtree(pool, org).await?)
}

/// The services and clients of the request must belong to the org or an org grouped under it.
pub(super) async fn check_scope(pool: &PgPool, org: Uuid, request: &SearchRequest) -> Result<(), ApiError> {
    let scope = subtree(pool, org).await?;
    let services: Vec<Uuid> = request.parties.iter().flat_map(|party| party.services.iter().map(|want| want.service.0)).collect();
    let clients: Vec<Uuid> = request.parties.iter().filter_map(|party| party.client.map(|client| client.0)).collect();
    for (table, ids) in [("services", services), ("clients", clients)] {
        let owners: Vec<(Uuid, Uuid)> = sqlx::query_as(&format!("SELECT id, org_id FROM {table} WHERE id = ANY($1) AND deleted_at IS NULL")).bind(&ids).fetch_all(pool).await?;
        let missing = ids.iter().find(|id| !owners.iter().any(|(found, owner)| found == *id && scope.contains(owner)));
        if let Some(id) = missing {
            return Err(ApiError::not_found(format!("{table} {id} not found")));
        }
    }
    Ok(())
}
