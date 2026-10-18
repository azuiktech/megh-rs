//! `/api/v1/orgs/{org_id}/waitlistEntries`: a client waits for a service in a window of time.
//! Joining is a create, leaving a delete; an entry is not changed. Entries are listed in the order they were requested.

use std::future::Future;

use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::PgPool;
use uuid::Uuid;

use super::common::instant;
use super::crud::{Collection, Context, Mask, Stamps};
use super::error::ApiError;
use crate::booking::{Attributes, ClientId, Interval, ServiceId, Target, WaitlistEntry, WaitlistEntryId, WaitlistEntryRow};
use crate::TableEntity;

pub(super) struct WaitlistEntries;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct WaitlistEntryBody {
    client_id: Option<ClientId>,
    target: Option<TargetBody>,
    request_time: Option<String>,
    attributes: Option<Attributes>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum TargetBody {
    Window(WindowBody),
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WindowBody {
    service_id: ServiceId,
    start_time: String,
    end_time: String,
}

fn target(body: &TargetBody, ctx: &Context) -> Result<Target, ApiError> {
    match body {
        TargetBody::Window(window) => {
            let during = Interval { start: instant(&window.start_time, ctx.zone)?, end: instant(&window.end_time, ctx.zone)? };
            (during.end > during.start).then_some(Target::Window(window.service_id, during)).ok_or_else(|| ApiError::invalid("end_time must be after start_time"))
        }
    }
}

fn target_json(target: &Target, ctx: &Context) -> Value {
    match target {
        Target::Window(service, during) => json!({"window": {"service_id": service, "start_time": during.start.with_timezone(&ctx.zone), "end_time": during.end.with_timezone(&ctx.zone)}}),
    }
}

/// The client, and the service waited for, must be covered by the org of the path.
async fn check_scope(ctx: &Context, client: ClientId, target: &Target) -> Result<(), ApiError> {
    let Target::Window(service, _) = target;
    let (table, id) = ("services", service.0);
    for (table, id) in [("clients", client.0), (table, id)] {
        let found: Option<Option<Uuid>> = sqlx::query_scalar(&format!("SELECT org_id FROM {table} WHERE id = $1 AND deleted_at IS NULL")).bind(id).fetch_optional(&ctx.pool).await?;
        match found.flatten() {
            Some(owner) if ctx.scope.contains(&owner) => {}
            _ => return Err(ApiError::not_found(format!("{table} {id} not found"))),
        }
    }
    Ok(())
}

async fn check_not_waiting(ctx: &Context, client: ClientId, target: &Target) -> Result<(), ApiError> {
    let query = "SELECT EXISTS (SELECT 1 FROM waitlist_entries WHERE client_id = $1 AND target = $2 AND deleted_at IS NULL)";
    let waiting: bool = sqlx::query_scalar(query).bind(client).bind(sqlx::types::Json(target)).fetch_one(&ctx.pool).await?;
    match waiting {
        true => Err(ApiError::conflict("the client is waiting for it already")),
        false => Ok(()),
    }
}

impl Collection for WaitlistEntries {
    type Row = WaitlistEntryRow;
    type Body = WaitlistEntryBody;
    const NAME: &'static str = "waitlistEntries";
    const TABLE: &'static str = "waitlist_entries";
    const ORDER: &'static str = "requested_at, created_at, id";

    fn stamps(row: &WaitlistEntryRow) -> Stamps {
        Stamps { id: row.id.0, created: row.created_at, updated: row.updated_at, deleted: row.deleted_at }
    }

    fn fields(row: &WaitlistEntryRow, ctx: &Context) -> Value {
        json!({"org_id": row.org_id, "client_id": row.client_id, "target": target_json(&row.target, ctx), "request_time": row.requested_at.with_timezone(&ctx.zone), "attributes": row.attributes})
    }

    async fn create(ctx: &Context, body: WaitlistEntryBody) -> Result<WaitlistEntryRow, ApiError> {
        let (Some(client), Some(wanted)) = (body.client_id, body.target.as_ref()) else { return Err(ApiError::invalid("client_id and target are required")) };
        let target = target(wanted, ctx)?;
        check_scope(ctx, client, &target).await?;
        check_not_waiting(ctx, client, &target).await?;
        let requested_at: DateTime<Utc> = body.request_time.as_deref().map(|text| instant(text, ctx.zone)).transpose()?.unwrap_or_else(Utc::now);
        let data = WaitlistEntry { org_id: Some(ctx.org), client_id: client, target, requested_at, attributes: body.attributes.unwrap_or_default() };
        Ok(WaitlistEntryRow::new(WaitlistEntryId(Uuid::new_v4()), data))
    }

    async fn update(_: &Context, _: &WaitlistEntryRow, _: WaitlistEntryBody, _: &Mask) -> Result<WaitlistEntryRow, ApiError> {
        Err(ApiError::invalid("a waitlist entry cannot be changed; delete it and join again"))
    }

    fn insert(pool: &PgPool, row: WaitlistEntryRow) -> impl Future<Output = Result<WaitlistEntryRow, sqlx::Error>> + Send {
        async move { row.insert(pool).await }
    }

    fn save(pool: &PgPool, row: WaitlistEntryRow) -> impl Future<Output = Result<WaitlistEntryRow, sqlx::Error>> + Send {
        async move { row.save(pool).await }
    }

    fn delete(pool: &PgPool, row: WaitlistEntryRow) -> impl Future<Output = Result<Option<WaitlistEntryRow>, sqlx::Error>> + Send {
        async move { row.soft_delete(pool).await }
    }
}
