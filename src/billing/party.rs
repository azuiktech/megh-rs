//! Whom a document is with: a client of the org for what the org sells, a vendor for what it buys.

use sqlx::PgPool;
use uuid::Uuid;

use crate::auth::rest::{ApiError, ApiResult};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Party {
    Client,
    Vendor,
}

impl Party {
    fn exists(self) -> &'static str {
        match self {
            Self::Client => "SELECT EXISTS (SELECT 1 FROM clients WHERE org_id = $1 AND id = $2 AND deleted_at IS NULL)",
            Self::Vendor => "SELECT EXISTS (SELECT 1 FROM billing_vendors WHERE org_id = $1 AND id = $2 AND deleted_at IS NULL)",
        }
    }
}

/// `Ok` when `id` is a live client or vendor of the org; a bad request otherwise.
pub async fn ensure_party(pool: &PgPool, org_id: Uuid, party: Party, id: Uuid) -> ApiResult<()> {
    let found: bool = sqlx::query_scalar(party.exists()).bind(org_id).bind(id).fetch_one(pool).await?;
    found.then_some(()).ok_or_else(|| ApiError::BadRequest(format!("party_id is not a {party:?} of the org").to_lowercase()))
}
