//! A slot posted back by a client is only a claim. The server searches again with the request it came from and uses
//! the slot it finds equal to the claim, so a crafted slot (another org's bookable, a shorter service) is never held.

use sqlx::PgPool;
use uuid::Uuid;

use super::availability::within_limits;
use super::common::check_scope;
use crate::auth::api::ApiError;
use crate::booking::{Search, SearchRequest, Slot, Slots};

/// The search request of the claim, for the org of the path, and the server's own slot that equals it, or `Invalid`
/// when there is none. `ignoring` is the appointment being moved, whose own time counts for nothing.
pub(super) async fn verified(pool: &PgPool, org: Uuid, mut request: SearchRequest, claimed: &Slot, ignoring: Option<Uuid>) -> Result<(SearchRequest, Slot), ApiError> {
    request.org = Some(org);
    within_limits(&request)?;
    check_scope(pool, org, &request).await?;
    let slots = Slots::new(pool.clone());
    let found = match ignoring {
        Some(id) => slots.find_ignoring(&request, id).await?,
        None => slots.find(&request).await?,
    };
    let slot = found.into_iter().find(|slot| slot == claimed);
    slot.map(|slot| (request, slot)).ok_or_else(|| ApiError::invalid("the slot is not available any more; search again"))
}
