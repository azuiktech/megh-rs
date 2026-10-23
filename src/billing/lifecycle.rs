//! Opening and voiding an invoice. Opening is one transaction: the invoice is locked, its lines are totalled, the
//! number is taken and the invoice is saved as open, so a failure leaves a draft and no number used.

use std::collections::HashMap;

use sqlx::{PgConnection, PgPool};
use uuid::Uuid;

use super::ids::{issue, Formats, Numbered};
use super::invoice::{Invoice, InvoiceData, InvoiceLine, InvoiceLineData, InvoiceStatus};
use super::tax::TaxData;
use super::totals::{self, Amounts, TotalsError};
use super::value::Percent;
use crate::auth::rest::{ApiError, ApiResult, Key};
use crate::Table;

/// A draft with a line becomes an open invoice with its number and totals; the amount due is the total.
pub async fn open(pool: &PgPool, formats: &Formats, key: &Key) -> ApiResult<Invoice> {
    let mut transaction = pool.begin().await?;
    let invoice = locked(&mut transaction, key).await?;
    (invoice.status == InvoiceStatus::Draft).then_some(()).ok_or_else(|| conflict("only a draft is opened"))?;
    let lines = lines_of(&mut transaction, key).await?;
    (!lines.is_empty()).then_some(()).ok_or_else(|| conflict("an invoice needs a line"))?;
    let amounts = line_amounts(&mut transaction, &invoice, &lines).await?;
    store_lines(&mut transaction, key.org_id, &lines, &amounts).await?;
    let number = issue(&mut transaction, key.org_id, Numbered::Invoice, invoice.date, formats.invoice.as_ref()).await?;
    let opened = store_invoice(&mut transaction, key, &number.to_string(), totals::invoice(&amounts)).await?;
    transaction.commit().await?;
    Ok(opened)
}

/// An open invoice nothing was paid on becomes void; it keeps its number.
pub async fn void(pool: &PgPool, key: &Key) -> ApiResult<Invoice> {
    let query = format!(
        "UPDATE {} SET status = 'void', updated_at = now() WHERE org_id = $1 AND id = $2 AND deleted_at IS NULL AND status = 'open' AND amount_paid = 0 RETURNING *",
        InvoiceData::TABLE_NAME
    );
    match sqlx::query_as(&query).bind(key.org_id).bind(key.id).fetch_optional(pool).await? {
        Some(voided) => Ok(voided),
        None => Err(why_not_voided(pool, key).await?),
    }
}

async fn why_not_voided(pool: &PgPool, key: &Key) -> ApiResult<ApiError> {
    let query = format!("SELECT EXISTS (SELECT 1 FROM {} WHERE org_id = $1 AND id = $2 AND deleted_at IS NULL)", InvoiceData::TABLE_NAME);
    let found: bool = sqlx::query_scalar(&query).bind(key.org_id).bind(key.id).fetch_one(pool).await?;
    Ok(if found { conflict("only an open invoice nothing was paid on is voided") } else { ApiError::NotFound })
}

fn conflict(message: &str) -> ApiError {
    ApiError::Conflict(message.into())
}

async fn locked(connection: &mut PgConnection, key: &Key) -> ApiResult<Invoice> {
    let query = format!("SELECT * FROM {} WHERE org_id = $1 AND id = $2 AND deleted_at IS NULL FOR UPDATE", InvoiceData::TABLE_NAME);
    sqlx::query_as(&query).bind(key.org_id).bind(key.id).fetch_optional(connection).await?.ok_or(ApiError::NotFound)
}

async fn lines_of(connection: &mut PgConnection, key: &Key) -> ApiResult<Vec<InvoiceLine>> {
    let query = format!("SELECT * FROM {} WHERE org_id = $1 AND invoice_id = $2 AND deleted_at IS NULL ORDER BY position, id", InvoiceLineData::TABLE_NAME);
    Ok(sqlx::query_as(&query).bind(key.org_id).bind(key.id).fetch_all(connection).await?)
}

/// What each line comes to, with the percents of its taxes, which must be live taxes of the org.
async fn line_amounts(connection: &mut PgConnection, invoice: &Invoice, lines: &[InvoiceLine]) -> ApiResult<Vec<Amounts>> {
    let ids: Vec<Uuid> = lines.iter().flat_map(|line| line.tax_ids.iter().copied()).collect();
    let query = format!("SELECT id, rate FROM {} WHERE org_id = $1 AND id = ANY($2) AND deleted_at IS NULL", TaxData::TABLE_NAME);
    let rates: HashMap<Uuid, Percent> = sqlx::query_as(&query).bind(invoice.org_id).bind(&ids).fetch_all(connection).await?.into_iter().collect();
    lines.iter().map(|line| amounts_of(invoice, line, &rates)).collect()
}

fn amounts_of(invoice: &Invoice, line: &InvoiceLine, rates: &HashMap<Uuid, Percent>) -> ApiResult<Amounts> {
    let taxes: Option<Vec<Percent>> = line.tax_ids.iter().map(|id| rates.get(id).copied()).collect();
    let taxes = taxes.ok_or_else(|| ApiError::BadRequest("a line names a tax that is not a tax of the org".into()))?;
    totals::line(&invoice.currency, line.quantity, line.unit_price, line.discount, &taxes).map_err(bad_total)
}

fn bad_total(error: TotalsError) -> ApiError {
    ApiError::BadRequest(error.to_string())
}

async fn store_lines(connection: &mut PgConnection, org_id: Uuid, lines: &[InvoiceLine], amounts: &[Amounts]) -> ApiResult<()> {
    let query = format!(
        "UPDATE {} AS l SET sub_total = v.sub_total, total = v.total, updated_at = now() \
         FROM UNNEST($2::uuid[], $3::bigint[], $4::bigint[]) AS v(id, sub_total, total) WHERE l.org_id = $1 AND l.id = v.id",
        InvoiceLineData::TABLE_NAME
    );
    let ids: Vec<Uuid> = lines.iter().map(|line| line.id).collect();
    let subs: Vec<i64> = amounts.iter().map(|amount| i64::from(amount.sub_total)).collect();
    let totals: Vec<i64> = amounts.iter().map(|amount| i64::from(amount.total)).collect();
    sqlx::query(&query).bind(org_id).bind(ids).bind(subs).bind(totals).execute(connection).await?;
    Ok(())
}

async fn store_invoice(connection: &mut PgConnection, key: &Key, number: &str, sum: Amounts) -> ApiResult<Invoice> {
    let query = format!(
        "UPDATE {} SET status = 'open', number = $3, sub_total = $4, tax_total = $5, total = $6, amount_paid = 0, balance_due = $6, updated_at = now() \
         WHERE org_id = $1 AND id = $2 RETURNING *",
        InvoiceData::TABLE_NAME
    );
    let stored = sqlx::query_as(&query)
        .bind(key.org_id)
        .bind(key.id)
        .bind(number)
        .bind(i64::from(sum.sub_total))
        .bind(i64::from(sum.tax_total))
        .bind(i64::from(sum.total))
        .fetch_one(connection)
        .await?;
    Ok(stored)
}
