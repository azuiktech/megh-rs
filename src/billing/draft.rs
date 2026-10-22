//! A document that is no longer a draft is not changed; a mistake is corrected by a credit note or a new order.

use sqlx::PgPool;
use uuid::Uuid;

use crate::auth::rest::{ApiError, ApiResult, Changes};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Document {
    Order,
    Invoice,
}

impl Document {
    fn status(self) -> &'static str {
        match self {
            Self::Order => "SELECT status FROM billing_orders WHERE org_id = $1 AND id = $2 AND deleted_at IS NULL",
            Self::Invoice => "SELECT status FROM billing_invoices WHERE org_id = $1 AND id = $2 AND deleted_at IS NULL",
        }
    }

    fn line_status(self) -> &'static str {
        match self {
            Self::Order => "SELECT d.status FROM billing_order_lines l JOIN billing_orders d ON d.org_id = l.org_id AND d.id = l.order_id \
                            WHERE l.org_id = $1 AND l.id = $2 AND l.deleted_at IS NULL AND d.deleted_at IS NULL",
            Self::Invoice => "SELECT d.status FROM billing_invoice_lines l JOIN billing_invoices d ON d.org_id = l.org_id AND d.id = l.invoice_id \
                              WHERE l.org_id = $1 AND l.id = $2 AND l.deleted_at IS NULL AND d.deleted_at IS NULL",
        }
    }
}

fn draft(status: Option<String>) -> ApiResult<()> {
    match status.as_deref() {
        None => Err(ApiError::NotFound),
        Some("draft") => Ok(()),
        Some(other) => Err(ApiError::Conflict(format!("the document is {other}, not a draft"))),
    }
}

/// `Ok` when the document is a draft; not found when it is missing, a conflict when it is anything else.
pub async fn ensure_draft(pool: &PgPool, document: Document, org_id: Uuid, id: Uuid) -> ApiResult<()> {
    draft(sqlx::query_scalar(document.status()).bind(org_id).bind(id).fetch_optional(pool).await?)
}

/// The same for the document of a line.
pub async fn ensure_line_draft(pool: &PgPool, document: Document, org_id: Uuid, line_id: Uuid) -> ApiResult<()> {
    draft(sqlx::query_scalar(document.line_status()).bind(org_id).bind(line_id).fetch_optional(pool).await?)
}

/// A bad request when a patch sets any of `fields`, which only a verb of the document may change.
pub fn refuse_changes(changes: &Changes, fields: &[&str]) -> ApiResult<()> {
    match fields.iter().find(|field| changes.changes.contains_key(**field)) {
        Some(field) => Err(ApiError::BadRequest(format!("{field} cannot be patched"))),
        None => Ok(()),
    }
}
