//! `/api/v1/orgs/{org_id}/paymentMethods`: passes, vouchers, coupons, insurance, cash and card. A method is paid with
//! prepaid (a balance of uses or money, optionally valid only for a window) or on the spot; `eligible` is an AIP-160
//! filter.

use std::future::Future;

use chrono::Utc;
use chrono_tz::Tz;
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::PgPool;
use uuid::Uuid;

use super::common::{amount_json, instant, AmountBody};
use super::crud::{updates, Collection, Context, Mask, Stamps};
use super::error::ApiError;
use crate::booking::{Attributes, ClientId, Filter, Interval, OrgId, PaymentMethod, PaymentMethodId, PaymentMethodRow, Settlement};
use crate::TableEntity;

pub(super) struct PaymentMethods;

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct PaymentMethodBody {
    org_id: Option<OrgId>,
    display_name: Option<String>,
    owner_id: Option<ClientId>,
    eligible: Option<String>,
    settlement: Option<SettlementBody>,
    attributes: Option<Attributes>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum SettlementBody {
    Prepaid { balance: AmountBody, valid: Option<WindowBody> },
    OnSpot,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WindowBody {
    start_time: String,
    end_time: String,
}

fn named(name: Option<String>) -> Result<String, ApiError> {
    name.filter(|name| !name.trim().is_empty()).ok_or_else(|| ApiError::invalid("display_name is required"))
}

fn owner(ctx: &Context, org: Option<OrgId>) -> Result<OrgId, ApiError> {
    let org = org.unwrap_or(ctx.org);
    ctx.scope.contains(&org).then_some(org).ok_or_else(|| ApiError::invalid("an org other than this one or those grouped under it cannot own a payment method"))
}

fn settlement(body: &SettlementBody, zone: Tz) -> Result<Settlement, ApiError> {
    match body {
        SettlementBody::OnSpot => Ok(Settlement::OnSpot),
        SettlementBody::Prepaid { balance, valid } => {
            let window = |window: &WindowBody| -> Result<Interval, ApiError> { Ok(Interval { start: instant(&window.start_time, zone)?, end: instant(&window.end_time, zone)? }) };
            let valid: Option<Interval> = valid.as_ref().map(window).transpose()?;
            if valid.is_some_and(|window| window.end <= window.start) {
                return Err(ApiError::invalid("valid must end after it starts"));
            }
            Ok(Settlement::Prepaid { balance: balance.amount()?, valid })
        }
    }
}

fn settlement_json(settlement: &Settlement, zone: Tz) -> Value {
    match settlement {
        Settlement::OnSpot => json!("on_spot"),
        Settlement::Prepaid { balance, valid } => {
            let window = valid.map(|window| json!({"start_time": window.start.with_timezone(&zone), "end_time": window.end.with_timezone(&zone)}));
            json!({"prepaid": {"balance": amount_json(balance), "valid": window}})
        }
    }
}

/// The owner, when given, is a client of the org's subtree.
async fn check_owner(ctx: &Context, client: Option<ClientId>) -> Result<(), ApiError> {
    let Some(client) = client else { return Ok(()) };
    let found: Option<Option<Uuid>> = sqlx::query_scalar("SELECT org_id FROM clients WHERE id = $1 AND deleted_at IS NULL").bind(client).fetch_optional(&ctx.pool).await?;
    match found {
        Some(org) if org.is_none_or(|org| ctx.scope.contains(&org)) => Ok(()),
        _ => Err(ApiError::not_found(format!("clients {} not found", client.0))),
    }
}

fn eligibility(text: &Option<String>) -> Result<crate::booking::Condition, ApiError> {
    let text = text.as_deref().ok_or_else(|| ApiError::invalid("eligible is required, an AIP-160 filter such as kind = \"massage\""))?;
    Ok(Box::new(Filter::parse(text)?))
}

fn text_of(condition: &crate::booking::Condition) -> Value {
    match serde_json::to_value(condition) {
        Ok(Value::Object(stored)) if stored.len() == 1 && stored.get("Filter").is_some_and(Value::is_string) => stored["Filter"].clone(),
        Ok(stored) => stored,
        Err(_) => Value::Null,
    }
}

impl Collection for PaymentMethods {
    type Row = PaymentMethodRow;
    type Body = PaymentMethodBody;
    const NAME: &'static str = "paymentMethods";
    const TABLE: &'static str = "payment_methods";

    fn stamps(row: &PaymentMethodRow) -> Stamps {
        Stamps { id: row.id.0, created: row.created_at, updated: row.updated_at, deleted: row.deleted_at }
    }

    fn fields(row: &PaymentMethodRow, ctx: &Context) -> Value {
        json!({"org_id": row.org_id, "display_name": row.name, "owner_id": row.owner_id, "eligible": text_of(&row.eligible), "settlement": settlement_json(&row.settlement, ctx.zone), "attributes": row.attributes})
    }

    async fn create(ctx: &Context, body: PaymentMethodBody) -> Result<PaymentMethodRow, ApiError> {
        check_owner(ctx, body.owner_id).await?;
        let data = PaymentMethod {
            org_id: Some(owner(ctx, body.org_id)?),
            name: named(body.display_name)?,
            owner_id: body.owner_id,
            eligible: eligibility(&body.eligible)?,
            settlement: settlement(body.settlement.as_ref().ok_or_else(|| ApiError::invalid("settlement is required"))?, ctx.zone)?,
            attributes: body.attributes.unwrap_or_default(),
        };
        Ok(PaymentMethodRow::new(PaymentMethodId(Uuid::new_v4()), data))
    }

    async fn update(ctx: &Context, current: &PaymentMethodRow, body: PaymentMethodBody, mask: &Mask) -> Result<PaymentMethodRow, ApiError> {
        let given = |field: &str, present: bool| updates(mask, field, present);
        if given("owner_id", body.owner_id.is_some()) {
            check_owner(ctx, body.owner_id).await?;
        }
        let data = PaymentMethod {
            org_id: if given("org_id", body.org_id.is_some()) { Some(owner(ctx, body.org_id)?) } else { current.org_id },
            name: if given("display_name", body.display_name.is_some()) { named(body.display_name)? } else { current.name.clone() },
            owner_id: if given("owner_id", body.owner_id.is_some()) { body.owner_id } else { current.owner_id },
            eligible: if given("eligible", body.eligible.is_some()) { eligibility(&body.eligible)? } else { reparsed(&current.eligible)? },
            settlement: if given("settlement", body.settlement.is_some()) { settlement(body.settlement.as_ref().ok_or_else(|| ApiError::invalid("settlement is required"))?, ctx.zone)? } else { current.settlement.clone() },
            attributes: if given("attributes", body.attributes.is_some()) { body.attributes.unwrap_or_default() } else { current.attributes.clone() },
        };
        Ok(PaymentMethodRow::with_timestamps(current.id, data, current.created_at, Utc::now()))
    }

    fn insert(pool: &PgPool, row: PaymentMethodRow) -> impl Future<Output = Result<PaymentMethodRow, sqlx::Error>> + Send {
        async move { row.insert(pool).await }
    }

    fn save(pool: &PgPool, row: PaymentMethodRow) -> impl Future<Output = Result<PaymentMethodRow, sqlx::Error>> + Send {
        async move { row.save(pool).await }
    }

    fn delete(pool: &PgPool, row: PaymentMethodRow) -> impl Future<Output = Result<Option<PaymentMethodRow>, sqlx::Error>> + Send {
        async move { row.soft_delete(pool).await }
    }
}

/// A copy of a stored condition (conditions are trait objects without `Clone`).
fn reparsed(condition: &crate::booking::Condition) -> Result<crate::booking::Condition, ApiError> {
    let invalid = |error: serde_json::Error| ApiError::invalid(error.to_string());
    serde_json::from_value(serde_json::to_value(condition).map_err(invalid)?).map_err(invalid)
}
