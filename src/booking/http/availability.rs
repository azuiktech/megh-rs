//! `POST /api/v1/orgs/{org}/availability:search`, a custom method (AIP-136): the slots where parties can be served.

use axum::extract::State;
use axum::response::{IntoResponse, Response};
use axum_content_negotiation::Negotiate;
use axum_params::Params;
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::common::check_scope;
use super::AppContext;
use crate::auth::api::{ApiError, ApiResult, ErrorBody};
use crate::booking::{AppointmentId, Search, SearchRequest, Slot, Slots};

const MOST_PARTIES: usize = 10;
const MOST_DAYS: i64 = 31;
const MOST_SLOTS: usize = 100;
const DEFAULT_HORIZON_DAYS: u32 = 180;
const MOST_HORIZON_DAYS: u32 = 366;

/// The request of a search, for the org of the path.
#[derive(Deserialize, utoipa::ToSchema)]
pub(super) struct SearchSlots {
    #[schema(value_type = String, format = Uuid)]
    org: Uuid,
    #[serde(flatten)]
    request: SearchRequest,
    /// How many days ahead to look for the next day with a slot when there is none; 180 when absent.
    horizon_days: Option<u32>,
    /// The appointment (or held appointment) being moved: its own time counts for nothing in this search.
    #[schema(value_type = Option<String>, format = Uuid)]
    reschedule: Option<AppointmentId>,
}

/// The slots found, and when there are none the first day after the window that has one.
#[derive(Serialize, utoipa::ToSchema)]
pub(super) struct FoundSlots {
    slots: Vec<Slot>,
    next_available_day: Option<NaiveDate>,
}

impl IntoResponse for FoundSlots {
    fn into_response(self) -> Response {
        Negotiate(self).into_response()
    }
}

pub(super) fn within_limits(request: &SearchRequest) -> Result<(), ApiError> {
    match (request.parties.len(), request.within.end - request.within.start) {
        (0, _) => Err(ApiError::invalid("give at least one party")),
        (count, _) if count > MOST_PARTIES => Err(ApiError::invalid(format!("give at most {MOST_PARTIES} parties"))),
        (_, span) if span <= chrono::Duration::zero() => Err(ApiError::invalid("within must end after it starts")),
        (_, span) if span > chrono::Duration::days(MOST_DAYS) => Err(ApiError::invalid(format!("the window is at most {MOST_DAYS} days"))),
        _ if request.parties.iter().any(|party| party.services.is_empty()) => Err(ApiError::invalid("a party needs at least one service")),
        _ => Ok(()),
    }
}

/// Find the slots where the parties can be served.
#[utoipa::path(post, path = "/api/v1/orgs/{org}/availability:search", tag = "availability", params(("org" = Uuid, Path)), request_body = SearchSlots,
    responses((status = OK, body = FoundSlots), (status = BAD_REQUEST, body = ErrorBody), (status = NOT_FOUND, body = ErrorBody)))]
pub(super) async fn search(State(ctx): State<AppContext>, Params(input, _): Params<SearchSlots>) -> ApiResult<FoundSlots> {
    let request = SearchRequest { org: Some(input.org), limit: Some(input.request.limit.unwrap_or(20).clamp(1, MOST_SLOTS)), ..input.request };
    within_limits(&request)?;
    check_scope(&ctx.pool, input.org, &request).await?;
    let slots = Slots::new(ctx.pool.clone());
    let found = match input.reschedule {
        Some(id) => slots.find_ignoring(&request, id.0).await?,
        None => slots.find(&request).await?,
    };
    let horizon = input.horizon_days.unwrap_or(DEFAULT_HORIZON_DAYS).min(MOST_HORIZON_DAYS);
    let next_available_day = match found.is_empty() {
        true => slots.next_day(&request, horizon).await?,
        false => None,
    };
    Ok(FoundSlots { slots: found, next_available_day })
}
