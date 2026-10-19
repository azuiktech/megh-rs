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
    first_name: Option<String>,
    last_name: Option<String>,
    email: Option<String>,
    home_phone: Option<String>,
    work_phone: Option<String>,
    mobile_phone: Option<String>,
    attributes: Option<Attributes>,
}

fn named(name: Option<String>) -> Result<String, ApiError> {
    name.filter(|name| !name.trim().is_empty()).ok_or_else(|| ApiError::invalid("display_name is required"))
}

fn part(name: Option<String>, field: &str) -> Result<String, ApiError> {
    name.filter(|name| !name.trim().is_empty()).ok_or_else(|| ApiError::invalid(format!("{field} is required")))
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
        json!({"org_id": row.org_id, "display_name": row.name, "first_name": row.first_name, "last_name": row.last_name, "email": row.email, "home_phone": row.home_phone, "work_phone": row.work_phone, "mobile_phone": row.mobile_phone, "attributes": row.attributes})
    }

    async fn create(ctx: &Context, body: ClientBody) -> Result<ClientRow, ApiError> {
        let data = Client {
            org_id: owner(ctx, body.org_id)?,
            name: named(body.display_name)?,
            first_name: part(body.first_name, "first_name")?,
            last_name: part(body.last_name, "last_name")?,
            email: body.email,
            home_phone: body.home_phone,
            work_phone: body.work_phone,
            mobile_phone: body.mobile_phone,
            attributes: body.attributes.unwrap_or_else(|| serde_json::json!({})),
        };
        Ok(ClientRow::new(ClientId(Uuid::new_v4()), data))
    }

    async fn update(ctx: &Context, current: &ClientRow, body: ClientBody, mask: &Mask) -> Result<ClientRow, ApiError> {
        let data = Client {
            org_id: if updates(mask, "org_id", body.org_id.is_some()) { owner(ctx, body.org_id)? } else { current.org_id },
            name: if updates(mask, "display_name", body.display_name.is_some()) { named(body.display_name)? } else { current.name.clone() },
            first_name: if updates(mask, "first_name", body.first_name.is_some()) { part(body.first_name, "first_name")? } else { current.first_name.clone() },
            last_name: if updates(mask, "last_name", body.last_name.is_some()) { part(body.last_name, "last_name")? } else { current.last_name.clone() },
            email: if updates(mask, "email", body.email.is_some()) { body.email } else { current.email.clone() },
            home_phone: if updates(mask, "home_phone", body.home_phone.is_some()) { body.home_phone } else { current.home_phone.clone() },
            work_phone: if updates(mask, "work_phone", body.work_phone.is_some()) { body.work_phone } else { current.work_phone.clone() },
            mobile_phone: if updates(mask, "mobile_phone", body.mobile_phone.is_some()) { body.mobile_phone } else { current.mobile_phone.clone() },
            attributes: if updates(mask, "attributes", body.attributes.is_some()) { body.attributes.unwrap_or_else(|| serde_json::json!({})) } else { current.attributes.clone() },
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
