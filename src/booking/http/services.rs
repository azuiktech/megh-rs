//! `/api/v1/orgs/{org_id}/services`: the catalogue. A service is made of steps; each step has a duration, whether the
//! client is present, and needs, written as AIP-160 filters over the tags of the bookables that can meet them.
//! Variants change the duration or price when their filter holds for a provider, room or client.

use std::future::Future;
use std::time::Duration;

use chrono::Utc;
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::PgPool;
use uuid::Uuid;

use super::common::{money, seconds, span_of, MoneyBody};
use super::crud::{updates, Collection, Context, Mask, Stamps};
use crate::auth::api::ApiError;
use crate::booking::{Attributes, Change, Condition, Engagement, Filter, Mode, Need, OrgId, Presence, Role, Service, ServiceId, ServiceRow, Step, Variant};
use crate::TableEntity;

pub(super) struct Services;

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ServiceBody {
    org_id: Option<OrgId>,
    display_name: Option<String>,
    mode: Option<Mode>,
    price: Option<MoneyBody>,
    steps: Option<Vec<StepBody>>,
    variants: Option<Vec<VariantBody>>,
    attributes: Option<Attributes>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct StepBody {
    duration: String,
    client: Option<Presence>,
    needs: Option<Vec<NeedBody>>,
    attributes: Option<Attributes>,
}

/// `who` is an AIP-160 filter over the tags of a bookable, such as `role = "stylist"`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct NeedBody {
    who: String,
    count: Option<u32>,
    engagement: Option<Engagement>,
    role: Option<Role>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ChangeBody<T> {
    Add(T),
    Set(T),
}

/// `when` is an AIP-160 filter over the tags of the provider, room or client.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct VariantBody {
    when: String,
    duration: Option<ChangeBody<String>>,
    price: Option<ChangeBody<MoneyBody>>,
}

fn named(name: Option<String>) -> Result<String, ApiError> {
    name.filter(|name| !name.trim().is_empty()).ok_or_else(|| ApiError::invalid("display_name is required"))
}

fn condition(text: &str) -> Result<Condition, ApiError> {
    Ok(Box::new(Filter::parse(text)?))
}

fn need(body: &NeedBody) -> Result<Need, ApiError> {
    let count = body.count.unwrap_or(1);
    let who = condition(&body.who)?;
    (count > 0).then_some(Need { who, count, engagement: body.engagement.unwrap_or(Engagement::Engaged), role: body.role.unwrap_or_default() }).ok_or_else(|| ApiError::invalid("a need is for at least 1"))
}

fn positive(text: &str) -> Result<Duration, ApiError> {
    Some(span_of(text)?).filter(|duration| !duration.is_zero()).ok_or_else(|| ApiError::invalid("a duration must be longer than 0s"))
}

fn step(body: &StepBody) -> Result<Step, ApiError> {
    let needs = body.needs.iter().flatten().map(need).collect::<Result<_, _>>()?;
    Ok(Step { duration: positive(&body.duration)?, client: body.client.unwrap_or(Presence::Present), needs, attributes: body.attributes.clone().unwrap_or_else(|| serde_json::json!({})) })
}

fn variant(body: &VariantBody) -> Result<Variant, ApiError> {
    let duration = body.duration.as_ref().map(|change| match change {
        ChangeBody::Add(text) => span_of(text).map(Change::Add),
        ChangeBody::Set(text) => positive(text).map(Change::Set),
    });
    let price = body.price.as_ref().map(|change| match change {
        ChangeBody::Add(money) => money.price().map(|price| Change::Add(price.micros)),
        ChangeBody::Set(money) => money.price().map(|price| Change::Set(price.micros)),
    });
    Ok(Variant { when: condition(&body.when)?, duration: duration.transpose()?, price_micros: price.transpose()? })
}

/// A filter shows as its text; any other condition as the JSON it is stored in.
fn text_of(condition: &Condition) -> Value {
    match serde_json::to_value(condition) {
        Ok(Value::Object(stored)) if stored.len() == 1 && stored.get("Filter").is_some_and(Value::is_string) => stored["Filter"].clone(),
        Ok(stored) => stored,
        Err(_) => Value::Null,
    }
}

fn step_json(step: &Step) -> Value {
    let needs: Vec<Value> = step.needs.iter().map(|n| json!({"who": text_of(&n.who), "count": n.count, "engagement": n.engagement})).collect();
    json!({"duration": seconds(step.duration), "client": step.client, "needs": needs, "attributes": step.attributes})
}

fn variant_json(variant: &Variant, currency: &str) -> Value {
    let price = |micros: i64| money(&crate::booking::Price { micros, currency: currency.to_string() });
    let duration = variant.duration.as_ref().map(|change| match change {
        Change::Add(d) => json!({"add": seconds(*d)}),
        Change::Set(d) => json!({"set": seconds(*d)}),
    });
    let amount = variant.price_micros.as_ref().map(|change| match change {
        Change::Add(micros) => json!({"add": price(*micros)}),
        Change::Set(micros) => json!({"set": price(*micros)}),
    });
    json!({"when": text_of(&variant.when), "duration": duration, "price": amount})
}

fn owner(ctx: &Context, org: Option<OrgId>) -> Result<OrgId, ApiError> {
    let org = org.unwrap_or(ctx.org);
    ctx.scope.contains(&org).then_some(org).ok_or_else(|| ApiError::invalid("an org other than this one or those grouped under it cannot own a service"))
}

fn built(ctx: &Context, body: ServiceBody) -> Result<Service, ApiError> {
    Ok(Service {
        org_id: owner(ctx, body.org_id)?,
        name: named(body.display_name)?,
        mode: body.mode.unwrap_or(Mode::Appointment),
        price: body.price.ok_or_else(|| ApiError::invalid("price is required"))?.price()?,
        steps: body.steps.iter().flatten().map(step).collect::<Result<_, _>>()?,
        variants: body.variants.iter().flatten().map(variant).collect::<Result<_, _>>()?,
        attributes: body.attributes.unwrap_or_else(|| serde_json::json!({})),
    })
}

impl Collection for Services {
    type Row = ServiceRow;
    type Body = ServiceBody;
    const NAME: &'static str = "services";
    const TABLE: &'static str = "services";

    fn stamps(row: &ServiceRow) -> Stamps {
        Stamps { id: row.id.0, created: row.created_at, updated: row.updated_at, deleted: row.deleted_at }
    }

    fn fields(row: &ServiceRow, _: &Context) -> Value {
        let steps: Vec<Value> = row.steps.iter().map(step_json).collect();
        let variants: Vec<Value> = row.variants.iter().map(|v| variant_json(v, &row.price.currency)).collect();
        json!({"org_id": row.org_id, "display_name": row.name, "mode": row.mode, "price": money(&row.price), "steps": steps, "variants": variants, "attributes": row.attributes})
    }

    async fn create(ctx: &Context, body: ServiceBody) -> Result<ServiceRow, ApiError> {
        Ok(ServiceRow::new(ServiceId(Uuid::new_v4()), built(ctx, body)?))
    }

    async fn update(ctx: &Context, current: &ServiceRow, body: ServiceBody, mask: &Mask) -> Result<ServiceRow, ApiError> {
        let given = |field: &str, present: bool| updates(mask, field, present);
        let data = Service {
            org_id: if given("org_id", body.org_id.is_some()) { owner(ctx, body.org_id)? } else { current.org_id },
            name: if given("display_name", body.display_name.is_some()) { named(body.display_name)? } else { current.name.clone() },
            mode: if given("mode", body.mode.is_some()) { body.mode.unwrap_or(Mode::Appointment) } else { current.mode },
            price: if given("price", body.price.is_some()) { body.price.ok_or_else(|| ApiError::invalid("price is required"))?.price()? } else { current.price.clone() },
            steps: if given("steps", body.steps.is_some()) { body.steps.iter().flatten().map(step).collect::<Result<_, _>>()? } else { reparsed(&current.steps)? },
            variants: if given("variants", body.variants.is_some()) { body.variants.iter().flatten().map(variant).collect::<Result<_, _>>()? } else { reparsed(&current.variants)? },
            attributes: if given("attributes", body.attributes.is_some()) { body.attributes.unwrap_or_else(|| serde_json::json!({})) } else { current.attributes.clone() },
        };
        Ok(ServiceRow::with_timestamps(current.id, data, current.created_at, Utc::now()))
    }

    fn insert(pool: &PgPool, row: ServiceRow) -> impl Future<Output = Result<ServiceRow, sqlx::Error>> + Send {
        async move { row.insert(pool).await }
    }

    fn save(pool: &PgPool, row: ServiceRow) -> impl Future<Output = Result<ServiceRow, sqlx::Error>> + Send {
        async move { row.save(pool).await }
    }

    fn delete(pool: &PgPool, row: ServiceRow) -> impl Future<Output = Result<Option<ServiceRow>, sqlx::Error>> + Send {
        async move { row.soft_delete(pool).await }
    }
}

/// A copy of the stored steps or variants (conditions are trait objects without `Clone`).
fn reparsed<T: serde::Serialize + serde::de::DeserializeOwned>(items: &[T]) -> Result<Vec<T>, ApiError> {
    let invalid = |error: serde_json::Error| ApiError::invalid(error.to_string());
    serde_json::from_value(serde_json::to_value(items).map_err(invalid)?).map_err(invalid)
}
