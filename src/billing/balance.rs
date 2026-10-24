//! The balance of an invoice follows its payments. Recording, changing or deleting a payment is one transaction that
//! also refreshes the invoice it pays: `amount_paid` is the sum of `amount` less `refunded` over its processing and
//! succeeded payments, and `balance_due` is the `total` less that. A payment never pays more than is owed.
//! The invoice row is locked first, so payments made at the same time are checked one after the other.

use sqlx::{PgConnection, PgPool};
use uuid::Uuid;
use validator::Validate;

use super::draft::refuse_changes;
use super::ids::{issue, Formats, Numbered};
use super::invoice::{Invoice, InvoiceData, InvoiceStatus};
use super::payment::{Payment, PaymentData};
use crate::auth::rest::{ApiError, ApiResult, Changes, Key, Keyed};
use crate::{Entity, Table, TableEntity};

/// A new payment for an open invoice (409 if it is not open, or if the payment is more than is owed): it takes its
/// number from the app's payment `Ids` (the scope is the payment's date), and the invoice is refreshed.
pub async fn record(pool: &PgPool, formats: &Formats, payment: PaymentData) -> ApiResult<Payment> {
    (payment.number.is_none()).then_some(()).ok_or_else(|| bad("the number is made when the payment is recorded"))?;
    let mut transaction = pool.begin().await?;
    let status = lock_invoice(&mut transaction, payment.org_id, payment.invoice_id).await?;
    open(status.ok_or_else(|| bad("invoice_id is not an invoice of the org"))?)?;
    let number = issue(&mut transaction, payment.org_id, Numbered::Payment, payment.date, formats.payment.as_ref()).await?;
    let numbered = PaymentData { number: Some(number), ..payment };
    let stored = TableEntity::insert(&Entity::new(Uuid::now_v7(), numbered), &mut *transaction).await?;
    refresh(&mut transaction, stored.org_id, stored.invoice_id).await?;
    transaction.commit().await?;
    Ok(stored)
}

/// A payment replaced; the invoice is refreshed (409 if the payment is now more than is owed). Its number and its
/// invoice stay.
pub async fn replace(pool: &PgPool, body: Keyed<PaymentData>) -> ApiResult<Payment> {
    let mut transaction = pool.begin().await?;
    let current = lock_payment(&mut transaction, &Key { org_id: body.data.org_id, id: body.id }).await?;
    (body.data.number.is_none()).then_some(()).ok_or_else(|| bad("number cannot be changed"))?;
    (body.data.invoice_id == current.invoice_id).then_some(()).ok_or_else(|| bad("a payment stays with its invoice"))?;
    let replaced = Entity::new(body.id, PaymentData { number: current.number.clone(), ..body.data });
    save(transaction, Entity { created_at: current.created_at, ..replaced }).await
}

/// A payment patched (its amount, status or `refunded`, say); the invoice is refreshed the same way.
pub async fn patch(pool: &PgPool, body: Changes) -> ApiResult<Payment> {
    let mut transaction = pool.begin().await?;
    let current = lock_payment(&mut transaction, &body.key).await?;
    refuse_changes(&body, &["number", "invoice_id"])?;
    let changed = current.patch(&body.changes.into()).map_err(|error| bad(&error.to_string()))?;
    changed.data.validate().map_err(|error| bad(&error.to_string()))?;
    save(transaction, changed).await
}

/// A payment deleted; the invoice is refreshed.
pub async fn remove(pool: &PgPool, key: &Key) -> ApiResult<Payment> {
    let mut transaction = pool.begin().await?;
    lock_payment(&mut transaction, key).await?;
    let query = format!("UPDATE {} SET deleted_at = now(), updated_at = now() WHERE org_id = $1 AND id = $2 RETURNING *", PaymentData::TABLE_NAME);
    let deleted: Payment = sqlx::query_as(&query).bind(key.org_id).bind(key.id).fetch_one(&mut *transaction).await?;
    refresh(&mut transaction, deleted.org_id, deleted.invoice_id).await?;
    transaction.commit().await?;
    Ok(deleted)
}

fn bad(message: &str) -> ApiError {
    ApiError::BadRequest(message.into())
}

fn open(status: InvoiceStatus) -> ApiResult<()> {
    (status == InvoiceStatus::Open).then_some(()).ok_or_else(|| ApiError::Conflict("the invoice is not open".into()))
}

/// Locks the invoice row and tells its status; `None` when the org has no such invoice.
async fn lock_invoice(connection: &mut PgConnection, org_id: Uuid, invoice_id: Uuid) -> ApiResult<Option<InvoiceStatus>> {
    let query = format!("SELECT status FROM {} WHERE org_id = $1 AND id = $2 AND deleted_at IS NULL FOR UPDATE", InvoiceData::TABLE_NAME);
    Ok(sqlx::query_scalar(&query).bind(org_id).bind(invoice_id).fetch_optional(connection).await?)
}

/// The payment, after its open invoice is locked (always in this order, so changes at once cannot deadlock).
async fn lock_payment(connection: &mut PgConnection, key: &Key) -> ApiResult<Payment> {
    let find = format!("SELECT invoice_id FROM {} WHERE org_id = $1 AND id = $2 AND deleted_at IS NULL", PaymentData::TABLE_NAME);
    let invoice_id: Uuid = sqlx::query_scalar(&find).bind(key.org_id).bind(key.id).fetch_optional(&mut *connection).await?.ok_or(ApiError::NotFound)?;
    open(lock_invoice(connection, key.org_id, invoice_id).await?.ok_or(ApiError::NotFound)?)?;
    let read = format!("SELECT * FROM {} WHERE org_id = $1 AND id = $2 AND deleted_at IS NULL FOR UPDATE", PaymentData::TABLE_NAME);
    sqlx::query_as(&read).bind(key.org_id).bind(key.id).fetch_optional(connection).await?.ok_or(ApiError::NotFound)
}

async fn save(mut transaction: sqlx::Transaction<'_, sqlx::Postgres>, payment: Payment) -> ApiResult<Payment> {
    let saved = TableEntity::update(&payment, &mut *transaction).await?;
    refresh(&mut transaction, saved.org_id, saved.invoice_id).await?;
    transaction.commit().await?;
    Ok(saved)
}

/// The invoice's `amount_paid` and `balance_due` from its payments, in the caller's transaction; a conflict when
/// the payments come to more than the invoice's total.
async fn refresh(connection: &mut PgConnection, org_id: Uuid, invoice_id: Uuid) -> ApiResult<Invoice> {
    let query = format!(
        "UPDATE {invoices} i SET amount_paid = p.paid, balance_due = i.total - p.paid, updated_at = now() \
         FROM (SELECT COALESCE(SUM(amount - refunded), 0)::bigint AS paid FROM {payments} \
               WHERE org_id = $1 AND invoice_id = $2 AND deleted_at IS NULL AND status IN ('processing', 'succeeded')) p \
         WHERE i.org_id = $1 AND i.id = $2 RETURNING i.*",
        invoices = InvoiceData::TABLE_NAME,
        payments = PaymentData::TABLE_NAME,
    );
    let invoice: Invoice = sqlx::query_as(&query).bind(org_id).bind(invoice_id).fetch_one(connection).await?;
    (i64::from(invoice.balance_due) >= 0).then_some(invoice).ok_or_else(|| ApiError::Conflict("the payments would come to more than is owed".into()))
}
