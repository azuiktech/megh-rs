//! `/api/v1/orgs/{org_id}/clients`: the people, pets and others who are served.

use std::future::Future;

use chrono::Utc;
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::PgPool;
use uuid::Uuid;

use super::crud::{updates, Collection, Context, Mask, Stamps};
use super::error::ApiError;
use crate::booking::{Attributes, Client, ClientId, ClientRow, OrgId};
use crate::TableEntity;

pub(super) struct Clients;

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ClientBody {
    org_id: Option<OrgId>,
    display_name: Option<String>,
    attributes: Option<Attributes>,
}

fn named(name: Option<String>) -> Result<String, ApiError> {
    name.filter(|name| !name.trim().is_empty()).ok_or_else(|| ApiError::invalid("display_name is required"))
}

/// A client belongs to the org of the path or to an org grouped under it.
fn owner(ctx: &Context, org: Option<OrgId>) -> Result<OrgId, ApiError> {
    let org = org.unwrap_or(ctx.org);
    ctx.scope.contains(&org).then_some(org).ok_or_else(|| ApiError::invalid("an org other than this one or those grouped under it cannot own a client"))
}

impl Collection for Clients {
    type Row = ClientRow;
    type Body = ClientBody;
    const NAME: &'static str = "clients";
    const TABLE: &'static str = "clients";

    fn stamps(row: &ClientRow) -> Stamps {
        Stamps { id: row.id.0, created: row.created_at, updated: row.updated_at, deleted: row.deleted_at }
    }

    fn fields(row: &ClientRow, _: &Context) -> Value {
        json!({"org_id": row.org_id, "display_name": row.name, "attributes": row.attributes})
    }

    async fn create(ctx: &Context, body: ClientBody) -> Result<ClientRow, ApiError> {
        let data = Client { org_id: Some(owner(ctx, body.org_id)?), name: named(body.display_name)?, attributes: body.attributes.unwrap_or_default() };
        Ok(ClientRow::new(ClientId(Uuid::new_v4()), data))
    }

    async fn update(ctx: &Context, current: &ClientRow, body: ClientBody, mask: &Mask) -> Result<ClientRow, ApiError> {
        let data = Client {
            org_id: if updates(mask, "org_id", body.org_id.is_some()) { Some(owner(ctx, body.org_id)?) } else { current.org_id },
            name: if updates(mask, "display_name", body.display_name.is_some()) { named(body.display_name)? } else { current.name.clone() },
            attributes: if updates(mask, "attributes", body.attributes.is_some()) { body.attributes.unwrap_or_default() } else { current.attributes.clone() },
        };
        Ok(ClientRow::with_timestamps(current.id, data, current.created_at, Utc::now()))
    }

    fn insert(pool: &PgPool, row: ClientRow) -> impl Future<Output = Result<ClientRow, sqlx::Error>> + Send {
        async move { row.insert(pool).await }
    }

    fn save(pool: &PgPool, row: ClientRow) -> impl Future<Output = Result<ClientRow, sqlx::Error>> + Send {
        async move { row.save(pool).await }
    }

    fn delete(pool: &PgPool, row: ClientRow) -> impl Future<Output = Result<Option<ClientRow>, sqlx::Error>> + Send {
        async move { row.soft_delete(pool).await }
    }
}
