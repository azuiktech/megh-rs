//! Row-change auditing: a trigger on a table writes each insert, update and delete, with the old and new row as
//! JSON, to an audit table. The tables and functions come from migration 0007; every name has a default and can
//! be overridden.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use uuid::Uuid;

use crate::JsonText;

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum AuditError {
    #[error(transparent)]
    Database(#[from] sqlx::Error),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[serde(rename_all = "UPPERCASE")]
#[sqlx(type_name = "text", rename_all = "UPPERCASE")]
pub enum AuditAction {
    Insert,
    Update,
    Delete,
}

/// One row of an audit table (megh-go's `audit_entries`). `old_data` and `new_data` are null where there is no such row.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, sqlx::FromRow)]
pub struct AuditEntry {
    pub id: Uuid,
    pub table_name: String,
    pub record_id: String,
    pub action: AuditAction,
    pub actor_id: Option<String>,
    #[sqlx(try_from = "Option<String>")]
    pub old_data: JsonText<serde_json::Value>,
    #[sqlx(try_from = "Option<String>")]
    pub new_data: JsonText<serde_json::Value>,
    pub created_at: Option<DateTime<Utc>>,
}

/// How one table is audited. By default changes go to the shared `audit_entries` table through a trigger named
/// `trg_<table>_audit`, keyed by the `id` column.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditTrigger {
    table: String,
    audit_table: String,
    name: String,
    key: String,
}

impl AuditTrigger {
    pub fn on(table: impl Into<String>) -> Self {
        let table = table.into();
        Self { name: format!("trg_{table}_audit"), audit_table: "audit_entries".into(), key: "id".into(), table }
    }

    /// Changes go to their own `<table>_audit` table instead of the shared one.
    pub fn dedicated(table: impl Into<String>) -> Self {
        let trigger = Self::on(table);
        let audit_table = format!("{}_audit", trigger.table);
        trigger.audit_table(audit_table)
    }

    pub fn audit_table(self, name: impl Into<String>) -> Self {
        Self { audit_table: name.into(), ..self }
    }

    pub fn name(self, name: impl Into<String>) -> Self {
        Self { name: name.into(), ..self }
    }

    /// The column that identifies a row in the history (the record id).
    pub fn key(self, column: impl Into<String>) -> Self {
        Self { key: column.into(), ..self }
    }
}

/// Installs audit triggers and reads their history; holds the injected pool.
#[derive(Clone)]
pub struct Audit {
    pool: PgPool,
}

impl Audit {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Creates the audit table and the trigger, each only if missing: a trigger of that name already on the table
    /// (for example megh-go's) is left as it is.
    pub async fn install(&self, trigger: &AuditTrigger) -> Result<(), AuditError> {
        sqlx::query("SELECT megh_audit_install($1, $2, $3, $4)")
            .bind(&trigger.table)
            .bind(&trigger.audit_table)
            .bind(&trigger.name)
            .bind(&trigger.key)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// The recorded changes of one row, oldest first.
    pub async fn history(&self, trigger: &AuditTrigger, record_id: &str) -> Result<Vec<AuditEntry>, AuditError> {
        Ok(sqlx::query_as("SELECT * FROM megh_audit_history($1, $2, $3)")
            .bind(&trigger.audit_table)
            .bind(&trigger.table)
            .bind(record_id)
            .fetch_all(&self.pool)
            .await?)
    }

    /// Attributes the changes made in this transaction to `actor`. Call it on the transaction's connection before
    /// the writes; the setting ends with the transaction, so it cannot leak to the next request on a pooled
    /// connection. Outside a transaction it has no lasting effect.
    pub async fn actor(connection: &mut sqlx::PgConnection, actor: &str) -> Result<(), AuditError> {
        sqlx::query("SELECT set_config('megh.actor_id', $1, true)").bind(actor).execute(connection).await?;
        Ok(())
    }
}
