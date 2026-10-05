//! The cascade of a soft delete: the rows a physical delete would remove through `ON DELETE CASCADE` foreign keys.

use std::collections::BTreeSet;

use sqlx::{PgConnection, Row};

/// A foreign key declared `ON DELETE CASCADE`: `child(child_columns)` references `parent(parent_columns)`.
struct Cascade {
    child: String,
    parent: String,
    child_columns: Vec<String>,
    parent_columns: Vec<String>,
}

const CASCADING_KEYS: &str = "
    SELECT cl.relname AS child, pl.relname AS parent,
           array_agg(ca.attname::text ORDER BY k.ord) AS child_columns,
           array_agg(pa.attname::text ORDER BY k.ord) AS parent_columns
    FROM pg_constraint con
    JOIN pg_class cl ON cl.oid = con.conrelid
    JOIN pg_class pl ON pl.oid = con.confrelid
    JOIN pg_namespace ns ON ns.oid = cl.relnamespace
    CROSS JOIN LATERAL unnest(con.conkey, con.confkey) WITH ORDINALITY AS k(child_attribute, parent_attribute, ord)
    JOIN pg_attribute ca ON ca.attrelid = con.conrelid AND ca.attnum = k.child_attribute
    JOIN pg_attribute pa ON pa.attrelid = con.confrelid AND pa.attnum = k.parent_attribute
    WHERE con.contype = 'f' AND con.confdeltype = 'c' AND ns.nspname = current_schema()
    GROUP BY con.oid, cl.relname, pl.relname";

const TABLES_WITH_TIMESTAMPS: &str = "
    SELECT table_name::text FROM information_schema.columns
    WHERE table_schema = current_schema() AND column_name IN ('deleted_at', 'updated_at')
    GROUP BY table_name HAVING count(*) = 2";

async fn cascading_keys(connection: &mut PgConnection) -> Result<Vec<Cascade>, sqlx::Error> {
    let rows = sqlx::query(CASCADING_KEYS).fetch_all(connection).await?;
    rows.iter()
        .map(|row| {
            Ok(Cascade {
                child: row.try_get("child")?,
                parent: row.try_get("parent")?,
                child_columns: row.try_get("child_columns")?,
                parent_columns: row.try_get("parent_columns")?,
            })
        })
        .collect()
}

/// The keys reachable from `root`: those whose parent is `root` or the child of a reachable key.
fn reachable(root: &str, keys: Vec<Cascade>) -> Vec<Cascade> {
    let mut tables = BTreeSet::from([root.to_string()]);
    let mut taken = vec![];
    let mut rest = keys;
    while let Some(next) = rest.iter().position(|key| tables.contains(&key.parent)) {
        let key = rest.remove(next);
        tables.insert(key.child.clone());
        taken.push(key);
    }
    taken
}

async fn require_timestamps(connection: &mut PgConnection, keys: &[Cascade]) -> Result<(), sqlx::Error> {
    let ready: Vec<String> = sqlx::query_scalar(TABLES_WITH_TIMESTAMPS).fetch_all(connection).await?;
    match keys.iter().find(|key| !ready.contains(&key.child)) {
        Some(key) => Err(sqlx::Error::Protocol(format!("soft delete: table `{}` is deleted along with `{}` but has no `deleted_at` and `updated_at`", key.child, key.parent))),
        None => Ok(()),
    }
}

/// Marks the children whose parent row was marked in this transaction (its `deleted_at` is the transaction's `now()`).
async fn mark_children(connection: &mut PgConnection, key: &Cascade) -> Result<u64, sqlx::Error> {
    let sql = format!(
        "UPDATE {child} SET deleted_at = now(), updated_at = now() WHERE deleted_at IS NULL AND ({child_columns}) IN (SELECT {parent_columns} FROM {parent} WHERE deleted_at = now())",
        child = key.child,
        parent = key.parent,
        child_columns = key.child_columns.join(", "),
        parent_columns = key.parent_columns.join(", "),
    );
    Ok(sqlx::query(&sql).execute(connection).await?.rows_affected())
}

/// Marks everything below the rows of `root` that were just marked, until nothing more changes.
pub(crate) async fn cascade(connection: &mut PgConnection, root: &str) -> Result<(), sqlx::Error> {
    let keys = reachable(root, cascading_keys(&mut *connection).await?);
    require_timestamps(&mut *connection, &keys).await?;
    loop {
        let mut marked = 0;
        for key in &keys {
            marked += mark_children(&mut *connection, key).await?;
        }
        if marked == 0 {
            return Ok(());
        }
    }
}
