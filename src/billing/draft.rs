//! An invoice that is no longer a draft is not changed; a mistake is corrected by voiding it and issuing another.
//! The tables are those named by the entities' `#[table]`.

use sqlx::PgPool;
use uuid::Uuid;

use crate::auth::rest::{ApiError, ApiResult, Changes};
use crate::Table;

/// A line of a document: the document it belongs to and the column that names it.
pub trait Line: Table {
    type Parent: Table;
    const PARENT_ID: &'static str;
}

fn draft(status: Option<String>) -> ApiResult<()> {
    match status.as_deref() {
        None => Err(ApiError::NotFound),
        Some("draft") => Ok(()),
        Some(other) => Err(ApiError::Conflict(format!("the document is {other}, not a draft"))),
    }
}

/// `Ok` when the document is a draft; not found when it is missing, a conflict when it is anything else.
pub async fn ensure_draft<T: Table>(pool: &PgPool, org_id: Uuid, id: Uuid) -> ApiResult<()> {
    let query = format!("SELECT status FROM {} WHERE org_id = $1 AND id = $2 AND deleted_at IS NULL", T::TABLE_NAME);
    draft(sqlx::query_scalar(&query).bind(org_id).bind(id).fetch_optional(pool).await?)
}

/// The same for the document of a line.
pub async fn ensure_line_draft<L: Line>(pool: &PgPool, org_id: Uuid, line_id: Uuid) -> ApiResult<()> {
    let query = format!(
        "SELECT d.status FROM {line} l JOIN {document} d ON d.org_id = l.org_id AND d.id = l.{parent} \
         WHERE l.org_id = $1 AND l.id = $2 AND l.deleted_at IS NULL AND d.deleted_at IS NULL",
        line = L::TABLE_NAME,
        document = <L::Parent as Table>::TABLE_NAME,
        parent = L::PARENT_ID,
    );
    draft(sqlx::query_scalar(&query).bind(org_id).bind(line_id).fetch_optional(pool).await?)
}

/// A bad request when a patch sets any of `fields`, which only a verb of the document may change.
pub fn refuse_changes(changes: &Changes, fields: &[&str]) -> ApiResult<()> {
    match fields.iter().find(|field| changes.changes.contains_key(**field)) {
        Some(field) => Err(ApiError::BadRequest(format!("{field} cannot be patched"))),
        None => Ok(()),
    }
}
