//! `/api/v1/orgs/{org}/groupings`: the edges of the hierarchy of orgs, bookables, services and clients (a brand
//! and its locations, a lobby and its rooms, a category and its services, a role and its people). A rule on a group
//! applies to its members. An edge is made and deleted, never changed. The member must be covered by the org of the
//! path (a location joins a brand from the location's own path); the group must be too, except that an org may join
//! any existing org.

use axum::extract::State;
use axum_params::Params;
use serde::Deserialize;
use sqlx::PgPool;
use uuid::Uuid;

use super::common::scope;
use super::page::paginate;
use super::AppContext;
use crate::auth::api::{ApiError, ApiResult, ErrorBody, ListQuery, Paginated};
use crate::booking::{Error, Grouped, Grouping, GroupingId, GroupingRow, Groups};
use crate::TableEntity;

#[derive(Deserialize)]
pub(super) struct ListGroupings {
    org: Uuid,
    #[serde(flatten)]
    query: ListQuery,
}

#[derive(Deserialize)]
pub(super) struct CreateGrouping {
    org: Uuid,
    #[serde(flatten)]
    grouping: Grouping,
}

#[derive(Deserialize)]
pub(super) struct GroupingRef {
    org: Uuid,
    id: Uuid,
}

/// The table and owner column of the entities of a kind.
fn table(kind: Grouped) -> (&'static str, &'static str) {
    match kind {
        Grouped::Org => ("organizations", "id"),
        Grouped::Bookable => ("bookables", "org_id"),
        Grouped::Service => ("services", "org_id"),
        Grouped::Client => ("clients", "org_id"),
    }
}

/// The live owner of an entity of the kind, or `None` when there is no such entity.
async fn owner_of(pool: &PgPool, kind: Grouped, id: Uuid) -> Result<Option<Uuid>, ApiError> {
    let (table, owner) = table(kind);
    let live = if kind == Grouped::Org { "" } else { " AND deleted_at IS NULL" };
    let found: Option<Option<Uuid>> = sqlx::query_scalar(&format!("SELECT {owner} FROM {table} WHERE id = $1{live}")).bind(id).fetch_optional(pool).await?;
    Ok(found.flatten())
}

/// The member must be an entity the org of the path covers: an org only places what is its own under a group.
async fn check_member(pool: &PgPool, scope: &[Uuid], kind: Grouped, id: Uuid) -> Result<(), ApiError> {
    match owner_of(pool, kind, id).await? {
        Some(owner) if scope.contains(&owner) => Ok(()),
        _ => Err(ApiError::not_found(format!("member {id} not found"))),
    }
}

/// The group is covered too, except for orgs: an org may be placed under any existing org.
async fn check_group(pool: &PgPool, scope: &[Uuid], kind: Grouped, id: Uuid) -> Result<(), ApiError> {
    match (owner_of(pool, kind, id).await?, kind) {
        (Some(_), Grouped::Org) => Ok(()),
        (Some(owner), _) if scope.contains(&owner) => Ok(()),
        _ => Err(ApiError::not_found(format!("group {id} not found"))),
    }
}

/// Adding the edge must not close a loop: the group must not already sit below the member.
async fn check_loop(pool: &PgPool, kind: Grouped, member: Uuid, group: Uuid) -> Result<(), ApiError> {
    let groups = Groups::load(pool, kind).await?;
    match groups.reach(group)?.contains_key(&member) {
        true => Err(Error::Cycle.into()),
        false => Ok(()),
    }
}

async fn found(pool: &PgPool, scope: &[Uuid], id: Uuid) -> ApiResult<GroupingRow> {
    let row = sqlx::query_as("SELECT * FROM groupings WHERE id = $1 AND org_id = ANY($2)").bind(id).bind(scope).fetch_optional(pool).await?;
    row.ok_or_else(|| ApiError::not_found(format!("grouping {id} not found")))
}

/// List groupings.
#[utoipa::path(get, path = "/api/v1/orgs/{org}/groupings", tag = "groupings", params(("org" = Uuid, Path), ListQuery),
    responses((status = OK, body = Paginated<GroupingRow>), (status = BAD_REQUEST, body = ErrorBody), (status = NOT_FOUND, body = ErrorBody)))]
pub(super) async fn list(State(ctx): State<AppContext>, Params(input, _): Params<ListGroupings>) -> ApiResult<Paginated<GroupingRow>> {
    let scope = scope(&ctx.pool, input.org).await?;
    let shown = if input.query.show_deleted == Some(true) { "" } else { " AND deleted_at IS NULL" };
    let rows = sqlx::query_as(&format!("SELECT * FROM groupings WHERE org_id = ANY($1){shown} ORDER BY created_at, id")).bind(&scope).fetch_all(&ctx.pool).await?;
    Ok(paginate(rows, &input.query)?)
}

/// Create a grouping: put a member under a group.
#[utoipa::path(post, path = "/api/v1/orgs/{org}/groupings", tag = "groupings", params(("org" = Uuid, Path)), request_body = Grouping,
    responses((status = OK, body = GroupingRow), (status = BAD_REQUEST, body = ErrorBody), (status = NOT_FOUND, body = ErrorBody), (status = CONFLICT, body = ErrorBody)))]
pub(super) async fn create(State(ctx): State<AppContext>, Params(input, _): Params<CreateGrouping>) -> ApiResult<GroupingRow> {
    let scope = scope(&ctx.pool, input.org).await?;
    let grouping = input.grouping;
    if !scope.contains(&grouping.org_id) {
        return Err(ApiError::invalid("an org other than this one or those grouped under it cannot own a grouping"));
    }
    if grouping.member_id == grouping.group_id {
        return Err(ApiError::invalid("an entity cannot be grouped under itself"));
    }
    check_member(&ctx.pool, &scope, grouping.kind, grouping.member_id).await?;
    check_group(&ctx.pool, &scope, grouping.kind, grouping.group_id).await?;
    check_loop(&ctx.pool, grouping.kind, grouping.member_id, grouping.group_id).await?;
    Ok(GroupingRow::new(GroupingId(Uuid::new_v4()), grouping).insert(&ctx.pool).await?)
}

/// Get a grouping.
#[utoipa::path(get, path = "/api/v1/orgs/{org}/groupings/{id}", tag = "groupings", params(("org" = Uuid, Path), ("id" = Uuid, Path)),
    responses((status = OK, body = GroupingRow), (status = BAD_REQUEST, body = ErrorBody), (status = NOT_FOUND, body = ErrorBody)))]
pub(super) async fn read(State(ctx): State<AppContext>, Params(input, _): Params<GroupingRef>) -> ApiResult<GroupingRow> {
    found(&ctx.pool, &scope(&ctx.pool, input.org).await?, input.id).await
}

/// Delete a grouping. Soft delete: it comes back with `deleted_at`.
#[utoipa::path(delete, path = "/api/v1/orgs/{org}/groupings/{id}", tag = "groupings", params(("org" = Uuid, Path), ("id" = Uuid, Path)),
    responses((status = OK, body = GroupingRow), (status = BAD_REQUEST, body = ErrorBody), (status = NOT_FOUND, body = ErrorBody)))]
pub(super) async fn remove(State(ctx): State<AppContext>, Params(input, _): Params<GroupingRef>) -> ApiResult<GroupingRow> {
    let current = found(&ctx.pool, &scope(&ctx.pool, input.org).await?, input.id).await?;
    if current.deleted_at.is_some() {
        return Err(ApiError::not_found(format!("grouping {} is deleted", input.id)));
    }
    current.soft_delete(&ctx.pool).await?.ok_or_else(|| ApiError::not_found("already deleted"))
}
