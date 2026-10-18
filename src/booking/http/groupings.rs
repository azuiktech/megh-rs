//! `/api/v1/orgs/{org_id}/groupings`: the edges of the hierarchy of orgs, bookables, services and clients (a brand
//! and its locations, a lobby and its rooms, a category and its services, a role and its people). A rule on a group
//! applies to its members. An edge is made and deleted, never changed. The member must be covered by the org of the
//! path (a location joins a brand from the location's own path); the group must be too, except that an org may join
//! any existing org.

use std::future::Future;

use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::PgPool;
use uuid::Uuid;

use super::crud::{Collection, Context, Mask, Stamps};
use super::error::ApiError;
use crate::booking::{Error, Grouped, Grouping, GroupingRow, Groups};
use crate::TableEntity;

pub(super) struct Groupings;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct GroupingBody {
    kind: Option<Grouped>,
    member_id: Option<Uuid>,
    group_id: Option<Uuid>,
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
async fn owner_of(ctx: &Context, kind: Grouped, id: Uuid) -> Result<Option<Uuid>, ApiError> {
    let (table, owner) = table(kind);
    let live = if kind == Grouped::Org { "" } else { " AND deleted_at IS NULL" };
    let found: Option<Option<Uuid>> = sqlx::query_scalar(&format!("SELECT {owner} FROM {table} WHERE id = $1{live}")).bind(id).fetch_optional(&ctx.pool).await?;
    Ok(found.flatten())
}

/// The member must be an entity the org of the path covers: an org only places what is its own under a group.
async fn check_member(ctx: &Context, kind: Grouped, id: Uuid) -> Result<(), ApiError> {
    match owner_of(ctx, kind, id).await? {
        Some(owner) if ctx.scope.contains(&owner) => Ok(()),
        _ => Err(ApiError::not_found(format!("member {id} not found"))),
    }
}

/// The group is covered too, except for orgs: an org may be placed under any existing org.
async fn check_group(ctx: &Context, kind: Grouped, id: Uuid) -> Result<(), ApiError> {
    match (owner_of(ctx, kind, id).await?, kind) {
        (Some(_), Grouped::Org) => Ok(()),
        (Some(owner), _) if ctx.scope.contains(&owner) => Ok(()),
        _ => Err(ApiError::not_found(format!("group {id} not found"))),
    }
}

/// Adding the edge must not close a loop: the group must not already sit below the member.
async fn check_loop(ctx: &Context, kind: Grouped, member: Uuid, group: Uuid) -> Result<(), ApiError> {
    let groups = Groups::load(&ctx.pool, kind).await?;
    match groups.reach(group)?.contains_key(&member) {
        true => Err(Error::Cycle.into()),
        false => Ok(()),
    }
}

impl Collection for Groupings {
    type Row = GroupingRow;
    type Body = GroupingBody;
    const NAME: &'static str = "groupings";
    const TABLE: &'static str = "groupings";

    fn stamps(row: &GroupingRow) -> Stamps {
        Stamps { id: row.id, created: row.created_at, updated: row.updated_at, deleted: row.deleted_at }
    }

    fn fields(row: &GroupingRow, _: &Context) -> Value {
        json!({"org_id": row.org_id, "kind": row.kind, "member_id": row.member_id, "group_id": row.group_id})
    }

    async fn create(ctx: &Context, body: GroupingBody) -> Result<GroupingRow, ApiError> {
        let (Some(kind), Some(member), Some(group)) = (body.kind, body.member_id, body.group_id) else { return Err(ApiError::invalid("kind, member_id and group_id are required")) };
        if member == group {
            return Err(ApiError::invalid("an entity cannot be grouped under itself"));
        }
        check_member(ctx, kind, member).await?;
        check_group(ctx, kind, group).await?;
        check_loop(ctx, kind, member, group).await?;
        Ok(GroupingRow::new(Uuid::new_v4(), Grouping { org_id: Some(ctx.org), kind, member_id: member, group_id: group }))
    }

    async fn update(_: &Context, _: &GroupingRow, _: GroupingBody, _: &Mask) -> Result<GroupingRow, ApiError> {
        Err(ApiError::invalid("a grouping cannot be changed; delete it and make another"))
    }

    fn insert(pool: &PgPool, row: GroupingRow) -> impl Future<Output = Result<GroupingRow, sqlx::Error>> + Send {
        async move { row.insert(pool).await }
    }

    fn save(pool: &PgPool, row: GroupingRow) -> impl Future<Output = Result<GroupingRow, sqlx::Error>> + Send {
        async move { row.save(pool).await }
    }

    fn delete(pool: &PgPool, row: GroupingRow) -> impl Future<Output = Result<Option<GroupingRow>, sqlx::Error>> + Send {
        async move { row.soft_delete(pool).await }
    }
}
