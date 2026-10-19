//! `/api/v1/orgs/{org_id}/bookables`: providers and rooms.

use std::future::Future;

use chrono::Utc;
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::PgPool;
use uuid::Uuid;

use super::crud::{updates, Collection, Context, Mask, Stamps};
use super::error::ApiError;
use crate::booking::{Attributes, Bookable, BookableId, BookableRow, OrgId};
use crate::TableEntity;

pub(super) struct Bookables;

/// A provider or a room. `capacity` above 1 is double booking; `attributes` are its tags.
#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct BookableBody {
    org_id: Option<OrgId>,
    display_name: Option<String>,
    capacity: Option<u32>,
    attributes: Option<Attributes>,
}

fn named(name: Option<String>) -> Result<String, ApiError> {
    name.filter(|name| !name.trim().is_empty()).ok_or_else(|| ApiError::invalid("display_name is required"))
}

fn sized(capacity: u32) -> Result<u32, ApiError> {
    Some(capacity).filter(|capacity| *capacity > 0).ok_or_else(|| ApiError::invalid("capacity must be at least 1"))
}

/// A bookable belongs to the org of the path or to an org grouped under it.
fn owner(ctx: &Context, org: Option<OrgId>) -> Result<OrgId, ApiError> {
    let org = org.unwrap_or(ctx.org);
    ctx.scope.contains(&org).then_some(org).ok_or_else(|| ApiError::invalid("an org other than this one or those grouped under it cannot own a bookable"))
}

impl Collection for Bookables {
    type Row = BookableRow;
    type Body = BookableBody;
    const NAME: &'static str = "bookables";
    const TABLE: &'static str = "bookables";

    fn stamps(row: &BookableRow) -> Stamps {
        Stamps { id: row.id.0, created: row.created_at, updated: row.updated_at, deleted: row.deleted_at }
    }

    fn fields(row: &BookableRow, _: &Context) -> Value {
        json!({"org_id": row.org_id, "display_name": row.name, "capacity": row.capacity, "attributes": row.attributes})
    }

    async fn create(ctx: &Context, body: BookableBody) -> Result<BookableRow, ApiError> {
        let data = Bookable { org_id: owner(ctx, body.org_id)?, name: named(body.display_name)?, capacity: sized(body.capacity.unwrap_or(1))?, attributes: body.attributes.unwrap_or_else(|| serde_json::json!({})) };
        Ok(BookableRow::new(BookableId(Uuid::new_v4()), data))
    }

    async fn update(ctx: &Context, current: &BookableRow, body: BookableBody, mask: &Mask) -> Result<BookableRow, ApiError> {
        let data = Bookable {
            org_id: if updates(mask, "org_id", body.org_id.is_some()) { owner(ctx, body.org_id)? } else { current.org_id },
            name: if updates(mask, "display_name", body.display_name.is_some()) { named(body.display_name)? } else { current.name.clone() },
            capacity: if updates(mask, "capacity", body.capacity.is_some()) { sized(body.capacity.unwrap_or(0))? } else { current.capacity },
            attributes: if updates(mask, "attributes", body.attributes.is_some()) { body.attributes.unwrap_or_else(|| serde_json::json!({})) } else { current.attributes.clone() },
        };
        Ok(BookableRow::with_timestamps(current.id, data, current.created_at, Utc::now()))
    }

    fn insert(pool: &PgPool, row: BookableRow) -> impl Future<Output = Result<BookableRow, sqlx::Error>> + Send {
        async move { row.insert(pool).await }
    }

    fn save(pool: &PgPool, row: BookableRow) -> impl Future<Output = Result<BookableRow, sqlx::Error>> + Send {
        async move { row.save(pool).await }
    }

    fn delete(pool: &PgPool, row: BookableRow) -> impl Future<Output = Result<Option<BookableRow>, sqlx::Error>> + Send {
        async move { row.soft_delete(pool).await }
    }
}
