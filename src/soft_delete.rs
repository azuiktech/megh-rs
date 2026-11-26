//! The cascade of a soft delete: the rows a physical delete would remove through `ON DELETE CASCADE` foreign keys.

use std::collections::BTreeSet;
use std::future::Future;
use std::pin::Pin;

use chrono::{DateTime, Utc};

use crate::auth::db::Tx;
use crate::auth::entity::Array;
use crate::Db;

/// A foreign key declared `ON DELETE CASCADE`: `child(child_columns)` references `parent(parent_columns)`.
#[derive(Clone, sqlx::FromRow)]
struct Cascade {
    child: String,
    parent: String,
    child_columns: Array<String>,
    parent_columns: Array<String>,
}

const POSTGRES_KEYS: &str = "
    SELECT cl.relname::text AS child, pl.relname::text AS parent,
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

const SQLITE_KEYS: &str = "
    SELECT child, parent, json_group_array(child_column) AS child_columns, json_group_array(parent_column) AS parent_columns
    FROM (SELECT m.name AS child, f.\"table\" AS parent, f.id AS id, f.\"from\" AS child_column, f.\"to\" AS parent_column
          FROM sqlite_master m JOIN pragma_foreign_key_list(m.name) f
          WHERE m.type = 'table' AND f.on_delete = 'CASCADE' ORDER BY m.name, f.id, f.seq)
    GROUP BY child, id";

const POSTGRES_STAMPED: &str = "
    SELECT table_name::text FROM information_schema.columns
    WHERE table_schema = current_schema() AND column_name IN ('deleted_at', 'updated_at')
    GROUP BY table_name HAVING count(*) = 2";

const SQLITE_STAMPED: &str = "
    SELECT m.name FROM sqlite_master m JOIN pragma_table_info(m.name) c
    WHERE m.type = 'table' AND c.name IN ('deleted_at', 'updated_at')
    GROUP BY m.name HAVING count(*) = 2";

/// The keys reachable from `root`: those whose parent is `root` or the child of a reachable key.
fn reachable(root: &str, keys: Vec<Cascade>) -> Vec<Cascade> {
    let start = (vec![], BTreeSet::from([root.to_string()]), keys);
    let (taken, _, _) = std::iter::successors(Some(start), |(taken, tables, rest)| {
        rest.iter().position(|key| tables.contains(&key.parent)).map(|next| {
            let (mut taken, mut tables, mut rest): (Vec<Cascade>, BTreeSet<String>, Vec<Cascade>) = (taken.clone(), tables.clone(), rest.clone());
            let key = rest.remove(next);
            tables.insert(key.child.clone());
            taken.push(key);
            (taken, tables, rest)
        })
    })
    .last()
    .expect("the start is a step");
    taken
}

async fn require_timestamps(tx: &mut Tx, keys: &[Cascade]) -> Result<(), sqlx::Error> {
    let ready: Vec<String> = Db::query_scalar(SQLITE_STAMPED).postgres(POSTGRES_STAMPED).fetch_all(&mut *tx).await?;
    match keys.iter().find(|key| !ready.contains(&key.child)) {
        Some(key) => Err(sqlx::Error::Protocol(format!("soft delete: table `{}` is deleted along with `{}` but has no `deleted_at` and `updated_at`", key.child, key.parent))),
        None => Ok(()),
    }
}

/// Marks the children whose parent row was marked at `now`.
async fn mark_children(tx: &mut Tx, key: &Cascade, now: DateTime<Utc>) -> Result<u64, sqlx::Error> {
    let sql = format!(
        "UPDATE \"{child}\" SET deleted_at = ?, updated_at = ? WHERE deleted_at IS NULL AND ({child_columns}) IN (SELECT {parent_columns} FROM \"{parent}\" WHERE deleted_at = ?)",
        child = key.child,
        parent = key.parent,
        child_columns = key.child_columns.join(", "),
        parent_columns = key.parent_columns.join(", "),
    );
    Db::query(sql).bind(now).bind(now).bind(now).execute(&mut *tx).await.map(|done| done.rows_affected())
}

type Marking<'a, T> = Pin<Box<dyn Future<Output = Result<T, sqlx::Error>> + Send + 'a>>;

/// Marks the children of each of `keys` once; how many rows it marked.
fn mark_each<'a>(tx: &'a mut Tx, keys: &'a [Cascade], now: DateTime<Utc>) -> Marking<'a, u64> {
    Box::pin(async move {
        match keys.split_first() {
            None => Ok(0),
            Some((key, rest)) => Ok(mark_children(&mut *tx, key, now).await? + mark_each(tx, rest, now).await?),
        }
    })
}

/// Marks the children of every key, again while any was marked.
fn mark_all<'a>(tx: &'a mut Tx, keys: &'a [Cascade], now: DateTime<Utc>) -> Marking<'a, ()> {
    Box::pin(async move {
        match mark_each(&mut *tx, keys, now).await? {
            0 => Ok(()),
            _ => mark_all(tx, keys, now).await,
        }
    })
}

/// Marks everything below the rows of `root` that were just marked at `now`, until nothing more changes.
pub(crate) async fn cascade(tx: &mut Tx, root: &str, now: DateTime<Utc>) -> Result<(), sqlx::Error> {
    let keys: Vec<Cascade> = Db::query_as(SQLITE_KEYS).postgres(POSTGRES_KEYS).fetch_all(&mut *tx).await?;
    let keys = reachable(root, keys);
    require_timestamps(&mut *tx, &keys).await?;
    mark_all(tx, &keys, now).await
}
