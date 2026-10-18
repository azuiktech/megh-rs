use std::collections::HashMap;

use petgraph::algo::toposort;
use petgraph::graphmap::DiGraphMap;
use sqlx::PgPool;
use uuid::Uuid;

use super::{Error, Grouped, Result};

/// The grouping graph of one entity kind, an edge from each member to a group it belongs to.
pub(crate) struct Groups(DiGraphMap<Uuid, ()>);

impl Groups {
    pub(crate) async fn load(pool: &PgPool, kind: Grouped) -> Result<Self> {
        let edges: Vec<(Uuid, Uuid)> = sqlx::query_as("SELECT member_id, group_id FROM groupings WHERE kind = $1 AND deleted_at IS NULL")
            .bind(kind)
            .fetch_all(pool)
            .await?;
        Ok(Self(DiGraphMap::from_edges(edges)))
    }

    /// `id` and every group above it, each with the length of the longest path to it.
    pub(crate) fn reach(&self, id: Uuid) -> Result<HashMap<Uuid, usize>> {
        let order = toposort(&self.0, None).map_err(|_| Error::Cycle)?;
        Ok(order.into_iter().fold(HashMap::from([(id, 0)]), |mut reach, node| {
            if let Some(&distance) = reach.get(&node) {
                self.0.neighbors(node).for_each(|group| {
                    let longest = reach.entry(group).or_insert(0);
                    *longest = (*longest).max(distance + 1);
                });
            }
            reach
        }))
    }
}
