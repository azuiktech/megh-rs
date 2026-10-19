//! What the REST resources share: the org's time zone, durations as AIP-142 writes them, and local times.

use std::time::Duration;

use chrono::{DateTime, NaiveDateTime, TimeZone, Utc};
use chrono_tz::Tz;
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::PgPool;
use uuid::Uuid;

use crate::auth::api::ApiError;
use super::subtree;
use crate::booking::{Amount, Price, SearchRequest};

/// `3600s`, with a fraction when needed (AIP-142).
pub(super) fn seconds(duration: Duration) -> String {
    match duration.subsec_nanos() {
        0 => format!("{}s", duration.as_secs()),
        nanos => format!("{}.{:09}s", duration.as_secs(), nanos).trim_end_matches('0').to_string() + "s",
    }
}

pub(super) async fn zone_of(pool: &PgPool, org: Uuid) -> Result<Tz, ApiError> {
    let found: Option<Option<String>> = sqlx::query_scalar("SELECT timezone FROM organizations WHERE id = $1").bind(org).fetch_optional(pool).await?;
    let name = found.ok_or_else(|| ApiError::not_found(format!("org {org} not found")))?;
    name.map_or(Ok(Tz::UTC), |name| name.parse().map_err(|_| ApiError::invalid(format!("the time zone {name} of the org is not known"))))
}

pub(super) fn span_of(text: &str) -> Result<Duration, ApiError> {
    humantime::parse_duration(text).map_err(|_| ApiError::invalid(format!("{text:?} is not a duration like 3600s")))
}

/// A time with an offset, or a local time (`2026-11-04T09:00:00`) read in the org's zone.
pub(super) fn instant(text: &str, zone: Tz) -> Result<DateTime<Utc>, ApiError> {
    let invalid = || ApiError::invalid(format!("{text:?} is not a time like 2026-11-04T09:00:00 or 2026-11-04T09:00:00-05:00"));
    if let Ok(with_offset) = DateTime::parse_from_rfc3339(text) {
        return Ok(with_offset.with_timezone(&Utc));
    }
    let local = NaiveDateTime::parse_from_str(text, "%Y-%m-%dT%H:%M:%S").map_err(|_| invalid())?;
    zone.from_local_datetime(&local).earliest().map(|time| time.with_timezone(&Utc)).ok_or_else(|| ApiError::invalid(format!("{text:?} does not exist in {}", zone.name())))
}

/// A price as `google.type.Money`: whole `units` (a string, as protobuf JSON writes an int64) and `nanos`.
pub(super) fn money(price: &Price) -> Value {
    let (units, micros) = (price.micros.div_euclid(1_000_000), price.micros.rem_euclid(1_000_000));
    json!({"currency_code": price.currency, "units": units.to_string(), "nanos": micros * 1000})
}

/// The services and clients of the request must belong to the org or an org grouped under it, or to no org.
pub(super) async fn check_scope(pool: &PgPool, org: Uuid, request: &SearchRequest) -> Result<(), ApiError> {
    let scope = subtree(pool, org).await?;
    let services: Vec<Uuid> = request.parties.iter().flat_map(|party| party.services.iter().map(|want| want.service.0)).collect();
    let clients: Vec<Uuid> = request.parties.iter().filter_map(|party| party.client.map(|client| client.0)).collect();
    for (table, ids) in [("services", services), ("clients", clients)] {
        let owners: Vec<(Uuid, Option<Uuid>)> = sqlx::query_as(&format!("SELECT id, org_id FROM {table} WHERE id = ANY($1) AND deleted_at IS NULL")).bind(&ids).fetch_all(pool).await?;
        let missing = ids.iter().find(|id| !owners.iter().any(|(found, owner)| found == *id && owner.is_none_or(|owner| scope.contains(&owner))));
        if let Some(id) = missing {
            return Err(ApiError::not_found(format!("{table} {id} not found")));
        }
    }
    Ok(())
}

/// `google.type.Money` as a client writes it: whole `units` (a string), and `nanos` that are whole thousands.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct MoneyBody {
    currency_code: String,
    units: String,
    nanos: i64,
}

impl MoneyBody {
    pub(super) fn price(&self) -> Result<Price, ApiError> {
        let bad = || ApiError::invalid("an amount of money is {currency_code, units, nanos}, not negative, with nanos in whole thousands");
        let units: i64 = self.units.parse().map_err(|_| bad())?;
        let micros = units.checked_mul(1_000_000).and_then(|whole| whole.checked_add(self.nanos / 1000)).filter(|micros| *micros >= 0 && self.nanos % 1000 == 0).ok_or_else(bad)?;
        Ok(Price { micros, currency: self.currency_code.clone() })
    }
}

/// What a payment or a balance is: a number of uses, or money.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum AmountBody {
    Uses(u32),
    Money(MoneyBody),
}

impl AmountBody {
    pub(super) fn amount(&self) -> Result<Amount, ApiError> {
        match self {
            Self::Uses(uses) => Ok(Amount::Uses(*uses)),
            Self::Money(money) => Ok(Amount::Money(money.price()?)),
        }
    }
}

pub(super) fn amount_json(amount: &Amount) -> Value {
    match amount {
        Amount::Uses(uses) => json!({"uses": uses}),
        Amount::Money(price) => json!({"money": money(price)}),
    }
}
