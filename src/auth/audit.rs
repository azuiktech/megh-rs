//! Row-change auditing: `crud` records each insert, update and delete in the table the entity's `AuditPolicy` names, in
//! the transaction of the change. The tables come from migration 0002 (the shared `audit_entries`, and `<table>_audit`
//! shaped like it).

use chrono::Utc;
use serde::Serialize;
use uuid::Uuid;

use crate::auth::db::Tx;
use crate::auth::entity::Table;
use crate::Db;

/// Where a table's changes are recorded; `Disabled` writes nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditPolicy {
    Disabled,
    /// The shared `audit_entries` table.
    Shared,
    /// `<table>_audit`.
    PerTable,
    /// The named table.
    Dedicated(&'static str),
}

impl AuditPolicy {
    pub fn table(self, entity: &str) -> Option<String> {
        match self {
            AuditPolicy::Disabled => None,
            AuditPolicy::Shared => Some("audit_entries".into()),
            AuditPolicy::PerTable => Some(format!("{entity}_audit")),
            AuditPolicy::Dedicated(name) => Some(name.into()),
        }
    }
}

/// Writes the change of one row, in the transaction of the change, as the entity's policy says.
pub(crate) async fn record<T: Table + Serialize>(tx: &mut Tx, action: &str, actor: Option<&str>, old: Option<&T>, new: Option<&T>) -> Result<(), sqlx::Error> {
    let Some(table) = T::AUDIT.table(T::TABLE_NAME) else { return Ok(()) };
    let snapshot = |entity: Option<&T>| entity.and_then(|entity| serde_json::to_value(entity).ok());
    let (old, new) = (snapshot(old), snapshot(new));
    let last = T::PRIMARY_KEY.last().copied().unwrap_or_default();
    let record_id = old.as_ref().or(new.as_ref()).and_then(|row| row.get(last)).map(|id| id.as_str().map_or_else(|| id.to_string(), str::to_owned));
    let insert = format!("INSERT INTO \"{table}\" (id, table_name, record_id, action, actor_id, old_data, new_data, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?)");
    Db::query(insert)
        .bind(Uuid::now_v7())
        .bind(T::TABLE_NAME.to_owned())
        .bind(record_id)
        .bind(action.to_owned())
        .bind(actor.map(str::to_owned))
        .bind(old.map(|row| row.to_string()))
        .bind(new.map(|row| row.to_string()))
        .bind(Utc::now())
        .execute(&mut *tx)
        .await
        .map(drop)
}
