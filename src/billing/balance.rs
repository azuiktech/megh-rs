//! The balance of an invoice follows its payments. Recording, changing or deleting a payment is one transaction that
//! also refreshes the invoice it pays: `amount_paid` is the sum of `amount` less `refunded` over its processing and
//! succeeded payments, and `balance_due` is the `total` less that. A payment never pays more than is owed.
//! The invoice row is locked first, so payments made at the same time are checked one after the other.
//! Stock follows too: while an invoice has a succeeded payment not refunded in full, its product lines are out of the
//! tenant's stock (receivable) or in it (payable). Selling more than is held is a 409.

use sqlx::{PgConnection, PgPool};
use uuid::Uuid;
use validator::Validate;

use super::draft::refuse_changes;
use super::ids::{issue, Formats, Numbered};
use super::invoice::{Invoice, InvoiceData, InvoiceStatus};
use super::invoice_line::InvoiceLineData;
use super::payment::{Payment, PaymentData};
use crate::auth::rest::{ApiError, ApiResult, Changes, Key, Keyed};
use crate::{Entity, Table, TableEntity};

/// A new payment for an open invoice (409 if it is not open, or if the payment is more than is owed): it takes its
/// number from the app's payment `Ids` (the scope is the payment's date), and the invoice is refreshed.
pub async fn record(pool: &PgPool, formats: &Formats, payment: PaymentData) -> ApiResult<Payment> {
    (payment.number.is_none()).then_some(()).ok_or_else(|| bad("the number is made when the payment is recorded"))?;
    let mut transaction = pool.begin().await?;
    let status = lock_invoice(&mut transaction, payment.tenant_id, payment.invoice_id).await?;
    open(status.ok_or_else(|| bad("invoice_id is not an invoice of the tenant"))?)?;
    let before = settled(&mut transaction, payment.tenant_id, payment.invoice_id).await?;
    let number = issue(&mut transaction, payment.tenant_id, Numbered::Payment, payment.date, formats.payment.as_ref()).await?;
    let numbered = PaymentData { number: Some(number), ..payment };
    let stored = TableEntity::insert(&Entity::new(Uuid::now_v7(), numbered), &mut *transaction).await?;
    refresh(&mut transaction, stored.tenant_id, stored.invoice_id).await?;
    restock(&mut transaction, stored.tenant_id, stored.invoice_id, before).await?;
    transaction.commit().await?;
    Ok(stored)
}

/// A payment replaced; the invoice is refreshed (409 if the payment is now more than is owed). Its number and its
/// invoice stay.
pub async fn replace(pool: &PgPool, body: Keyed<PaymentData>) -> ApiResult<Payment> {
    let mut transaction = pool.begin().await?;
    let current = lock_payment(&mut transaction, &Key { tenant_id: body.data.tenant_id, id: body.id }).await?;
    let before = settled(&mut transaction, current.tenant_id, current.invoice_id).await?;
    (body.data.number.is_none()).then_some(()).ok_or_else(|| bad("number cannot be changed"))?;
    (body.data.invoice_id == current.invoice_id).then_some(()).ok_or_else(|| bad("a payment stays with its invoice"))?;
    let replaced = Entity::new(body.id, PaymentData { number: current.number.clone(), ..body.data });
    save(transaction, Entity { created_at: current.created_at, ..replaced }, before).await
}

/// A payment patched (its amount, status or `refunded`, say); the invoice is refreshed the same way.
pub async fn patch(pool: &PgPool, body: Changes) -> ApiResult<Payment> {
    let mut transaction = pool.begin().await?;
    let current = lock_payment(&mut transaction, &body.key).await?;
    let before = settled(&mut transaction, current.tenant_id, current.invoice_id).await?;
    refuse_changes(&body, &["number", "invoice_id"])?;
    let changed = current.patch(&body.changes.into()).map_err(|error| bad(&error.to_string()))?;
    changed.data.validate().map_err(|error| bad(&error.to_string()))?;
    save(transaction, changed, before).await
}

/// A payment deleted; the invoice is refreshed.
pub async fn remove(pool: &PgPool, key: &Key) -> ApiResult<Payment> {
    let mut transaction = pool.begin().await?;
    let current = lock_payment(&mut transaction, key).await?;
    let before = settled(&mut transaction, current.tenant_id, current.invoice_id).await?;
    let query = format!("UPDATE {} SET deleted_at = now(), updated_at = now() WHERE tenant_id = $1 AND id = $2 RETURNING *", PaymentData::TABLE_NAME);
    let deleted: Payment = sqlx::query_as(&query).bind(key.tenant_id).bind(key.id).fetch_one(&mut *transaction).await?;
    refresh(&mut transaction, deleted.tenant_id, deleted.invoice_id).await?;
    restock(&mut transaction, deleted.tenant_id, deleted.invoice_id, before).await?;
    transaction.commit().await?;
    Ok(deleted)
}

fn bad(message: &str) -> ApiError {
    ApiError::BadRequest(message.into())
}

fn open(status: InvoiceStatus) -> ApiResult<()> {
    (status == InvoiceStatus::Open).then_some(()).ok_or_else(|| ApiError::Conflict("the invoice is not open".into()))
}

/// Locks the invoice row and tells its status; `None` when the tenant has no such invoice.
async fn lock_invoice(connection: &mut PgConnection, tenant_id: Uuid, invoice_id: Uuid) -> ApiResult<Option<InvoiceStatus>> {
    let query = format!("SELECT status FROM {} WHERE tenant_id = $1 AND id = $2 AND deleted_at IS NULL FOR UPDATE", InvoiceData::TABLE_NAME);
    Ok(sqlx::query_scalar(&query).bind(tenant_id).bind(invoice_id).fetch_optional(connection).await?)
}

/// The payment, after its open invoice is locked (always in this order, so changes at once cannot deadlock).
async fn lock_payment(connection: &mut PgConnection, key: &Key) -> ApiResult<Payment> {
    let find = format!("SELECT invoice_id FROM {} WHERE tenant_id = $1 AND id = $2 AND deleted_at IS NULL", PaymentData::TABLE_NAME);
    let invoice_id: Uuid = sqlx::query_scalar(&find).bind(key.tenant_id).bind(key.id).fetch_optional(&mut *connection).await?.ok_or(ApiError::NotFound)?;
    open(lock_invoice(connection, key.tenant_id, invoice_id).await?.ok_or(ApiError::NotFound)?)?;
    let read = format!("SELECT * FROM {} WHERE tenant_id = $1 AND id = $2 AND deleted_at IS NULL FOR UPDATE", PaymentData::TABLE_NAME);
    sqlx::query_as(&read).bind(key.tenant_id).bind(key.id).fetch_optional(connection).await?.ok_or(ApiError::NotFound)
}

async fn save(mut transaction: sqlx::Transaction<'_, sqlx::Postgres>, payment: Payment, before: bool) -> ApiResult<Payment> {
    let saved = TableEntity::update(&payment, &mut *transaction).await?;
    refresh(&mut transaction, saved.tenant_id, saved.invoice_id).await?;
    restock(&mut transaction, saved.tenant_id, saved.invoice_id, before).await?;
    transaction.commit().await?;
    Ok(saved)
}

/// The invoice's `amount_paid` and `balance_due` from its payments, in the caller's transaction; a conflict when
/// the payments come to more than the invoice's total.
async fn refresh(connection: &mut PgConnection, tenant_id: Uuid, invoice_id: Uuid) -> ApiResult<Invoice> {
    let query = format!(
        "UPDATE {invoices} i SET amount_paid = p.paid, balance_due = i.total - p.paid, updated_at = now() \
         FROM (SELECT COALESCE(SUM(amount - refunded), 0)::bigint AS paid FROM {payments} \
               WHERE tenant_id = $1 AND invoice_id = $2 AND deleted_at IS NULL AND status IN ('processing', 'succeeded')) p \
         WHERE i.tenant_id = $1 AND i.id = $2 RETURNING i.*",
        invoices = InvoiceData::TABLE_NAME,
        payments = PaymentData::TABLE_NAME,
    );
    let invoice: Invoice = sqlx::query_as(&query).bind(tenant_id).bind(invoice_id).fetch_one(connection).await?;
    (i64::from(invoice.balance_due) >= 0).then_some(invoice).ok_or_else(|| ApiError::Conflict("the payments would come to more than is owed".into()))
}

/// Whether the invoice has a succeeded payment not refunded in full, so its products have moved.
async fn settled(connection: &mut PgConnection, tenant_id: Uuid, invoice_id: Uuid) -> ApiResult<bool> {
    let query = format!(
        "SELECT EXISTS (SELECT 1 FROM {} WHERE tenant_id = $1 AND invoice_id = $2 AND deleted_at IS NULL \
         AND status = 'succeeded' AND refunded < amount)",
        PaymentData::TABLE_NAME
    );
    Ok(sqlx::query_scalar(&query).bind(tenant_id).bind(invoice_id).fetch_one(connection).await?)
}

/// Moves the invoice's product lines in or out of the tenant's stock when the payments settle or unsettle it:
/// a receivable invoice takes them out, a payable one puts them in, and undoing does the opposite.
async fn restock(connection: &mut PgConnection, tenant_id: Uuid, invoice_id: Uuid, before: bool) -> ApiResult<()> {
    let after = settled(connection, tenant_id, invoice_id).await?;
    let direction: i32 = match (before, after) {
        (false, true) => 1,
        (true, false) => -1,
        _ => return Ok(()),
    };
    let query = format!(
        "WITH sold AS (SELECT CASE kind WHEN 'payable' THEN $3 ELSE -$3 END AS sign FROM {invoices} WHERE tenant_id = $1 AND id = $2), \
              used AS (SELECT product_id, SUM(quantity) AS quantity FROM {lines} \
                       WHERE tenant_id = $1 AND invoice_id = $2 AND product_id IS NOT NULL AND deleted_at IS NULL GROUP BY product_id) \
         INSERT INTO stocks (tenant_id, id, product_id, on_hand) SELECT $1, uuidv7(), product_id, quantity * sign FROM used, sold \
         ON CONFLICT (tenant_id, product_id) DO UPDATE SET on_hand = stocks.on_hand + EXCLUDED.on_hand, updated_at = now() \
         RETURNING on_hand < 0",
        invoices = InvoiceData::TABLE_NAME,
        lines = InvoiceLineData::TABLE_NAME,
    );
    let short: Vec<bool> = sqlx::query_scalar(&query).bind(tenant_id).bind(invoice_id).bind(direction).fetch_all(connection).await?;
    (!short.contains(&true)).then_some(()).ok_or_else(|| ApiError::Conflict("not enough of a product on hand".into()))
}
