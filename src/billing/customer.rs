//! Whom an org may invoice: an org that belongs to it (its `parent_id` is the seller). The org of each client is made
//! with the client, under the business that has the client, so a client can always be invoiced.

use sqlx::PgPool;
use uuid::Uuid;

use crate::auth::rest::{ApiError, ApiResult};

/// `Ok` when `customer` is an org whose parent is `seller`; a bad request otherwise.
pub async fn ensure_customer(pool: &PgPool, seller: Uuid, customer: Uuid) -> ApiResult<()> {
    let found: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM organizations WHERE id = $1 AND parent_id = $2)")
        .bind(customer)
        .bind(seller)
        .fetch_one(pool)
        .await?;
    found.then_some(()).ok_or_else(|| ApiError::BadRequest("customer_id is not an org of the seller".into()))
}
