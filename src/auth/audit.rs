//! Row-change auditing: `crud` records each insert, update and delete in the table the entity's `AuditPolicy` names, in
//! the transaction of the change. The tables come from migration 0002 (the shared `audit_entries`, and `<table>_audit`
//! shaped like it).

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
pub(crate) async fn record<T: crate::Table + serde::Serialize, DB: crate::auth::db::Backend>(
    tx: &mut sqlx::Transaction<'_, DB>,
    action: &str,
    actor: Option<&str>,
    old: Option<&T>,
    new: Option<&T>,
) -> Result<(), sqlx::Error> {
    use sea_query::{Alias, Expr, Query};

    let Some(table) = T::AUDIT.table(T::TABLE_NAME) else { return Ok(()) };
    let snapshot = |entity: Option<&T>| entity.and_then(|entity| serde_json::to_string(entity).ok());
    let last = T::PRIMARY_KEY.last();
    let id = old.or(new).and_then(|entity| entity.values().into_iter().find(|(name, _)| Some(name) == last));
    let record_id = id.and_then(|(_, value)| <uuid::Uuid as crate::auth::column::Column>::from_value(value).ok()).map(|id| id.to_string());
    let columns = ["id", "table_name", "record_id", "action", "actor_id", "old_data", "new_data", "created_at"];
    let insert = Query::insert()
        .into_table(Alias::new(table))
        .columns(columns.map(Alias::new))
        .values_panic([
            Expr::val(uuid::Uuid::now_v7()).into(),
            Expr::val(T::TABLE_NAME).into(),
            Expr::val(record_id).into(),
            Expr::val(action).into(),
            Expr::val(actor).into(),
            Expr::val(snapshot(old)).into(),
            Expr::val(snapshot(new)).into(),
            DB::now(),
        ])
        .to_owned();
    DB::execute(&mut *tx, DB::build(&insert)).await.map(drop)
}
