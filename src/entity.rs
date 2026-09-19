//! Generic persistent entity wrapper and automated TableEntity operations.

use std::ops::{Deref, DerefMut};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Detailed column metadata extracted at compile time by #[derive(Table)].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ColumnMeta {
    pub name: &'static str,
    pub is_primary_key: bool,
    pub is_json: bool,
    pub has_default: bool,
}

/// Trait for types mapping to a database table with compile-time metadata.
pub trait Table {
    const TABLE_NAME: &'static str;
    const PRIMARY_KEY: &'static [&'static str] = &["id"];
    const CONFLICT_COLUMNS: &'static [&'static str] = Self::PRIMARY_KEY;
    const COLUMNS: &'static [ColumnMeta];

    /// Returns non-primary-key column names updated on conflict or save.
    fn update_columns() -> Vec<&'static str> {
        Self::COLUMNS
            .iter()
            .filter(|c| !c.is_primary_key && c.name != "created_at")
            .map(|c| c.name)
            .collect()
    }
}

/// Generic persistent entity wrapper providing ID and audit timestamps.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "postgres", derive(sqlx::FromRow))]
pub struct Entity<ID, T> {
    pub id: ID,
    #[serde(flatten)]
    #[cfg_attr(feature = "postgres", sqlx(flatten))]
    pub data: T,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl<ID, T> Deref for Entity<ID, T> {
    type Target = T;
    fn deref(&self) -> &Self::Target { &self.data }
}

impl<ID, T> DerefMut for Entity<ID, T> {
    fn deref_mut(&mut self) -> &mut Self::Target { &mut self.data }
}

impl<ID, T> Entity<ID, T> {
    pub fn new(id: ID, data: T) -> Self {
        let now = Utc::now();
        Self { id, data, created_at: now, updated_at: now }
    }

    pub fn with_timestamps(id: ID, data: T, created_at: DateTime<Utc>, updated_at: DateTime<Utc>) -> Self {
        Self { id, data, created_at, updated_at }
    }

    pub fn map<U>(self, f: impl FnOnce(T) -> U) -> Entity<ID, U> {
        Entity { id: self.id, data: f(self.data), created_at: self.created_at, updated_at: self.updated_at }
    }

    pub fn patch(&self, patch: &serde_json::Value) -> Result<Self, serde_json::Error>
    where
        T: Serialize + for<'de> Deserialize<'de>,
        ID: Clone,
    {
        let mut target = serde_json::to_value(&self.data)?;
        json_patch::merge(&mut target, patch);
        let updated_data: T = serde_json::from_value(target)?;
        Ok(Entity { id: self.id.clone(), data: updated_data, created_at: self.created_at, updated_at: Utc::now() })
    }
}

impl<ID, T: Table> Table for Entity<ID, T> {
    const TABLE_NAME: &'static str = T::TABLE_NAME;
    const PRIMARY_KEY: &'static [&'static str] = T::PRIMARY_KEY;
    const CONFLICT_COLUMNS: &'static [&'static str] = T::CONFLICT_COLUMNS;
    const COLUMNS: &'static [ColumnMeta] = T::COLUMNS;
}

#[cfg(feature = "postgres")]
impl<ID, T: Table> Entity<ID, T> {
    pub async fn upsert(&self, pool: &sqlx::PgPool) -> Result<Self, sqlx::Error>
    where
        Self: for<'r> sqlx::FromRow<'r, sqlx::postgres::PgRow> + Serialize + Send + Unpin,
    {
        TableEntity::upsert(self, pool).await
    }

    pub async fn insert(&self, pool: &sqlx::PgPool) -> Result<Self, sqlx::Error>
    where
        Self: for<'r> sqlx::FromRow<'r, sqlx::postgres::PgRow> + Serialize + Send + Unpin,
    {
        TableEntity::insert(self, pool).await
    }

    pub async fn save(&self, pool: &sqlx::PgPool) -> Result<Self, sqlx::Error>
    where
        Self: for<'r> sqlx::FromRow<'r, sqlx::postgres::PgRow> + Serialize + Send + Unpin,
    {
        TableEntity::update(self, pool).await
    }
}

#[cfg(feature = "postgres")]
#[allow(async_fn_in_trait)]
/// Automated database operations for any entity implementing Table.
pub trait TableEntity: Table + Serialize + for<'r> sqlx::FromRow<'r, sqlx::postgres::PgRow> + Send + Unpin {
    async fn upsert(&self, pool: &sqlx::PgPool) -> Result<Self, sqlx::Error> where Self: Sized {
        let table = Self::TABLE_NAME;
        let pks = Self::PRIMARY_KEY.join(", ");
        let cols = Self::update_columns();
        let set = cols.join(", ");
        let excl = cols.iter().map(|c| format!("EXCLUDED.{c}")).collect::<Vec<_>>().join(", ");
        let q = format!("INSERT INTO {table} SELECT * FROM json_populate_record(NULL::{table}, $1::json) ON CONFLICT ({pks}) DO UPDATE SET ({set}) = ({excl}) RETURNING *");
        sqlx::query_as::<_, Self>(&q).bind(sqlx::types::Json(self)).fetch_one(pool).await
    }

    async fn update(&self, pool: &sqlx::PgPool) -> Result<Self, sqlx::Error> where Self: Sized {
        let table = Self::TABLE_NAME;
        let cols = Self::update_columns();
        let set = cols.join(", ");
        let p_set = cols.iter().map(|c| format!("p.{c}")).collect::<Vec<_>>().join(", ");
        let where_clause = Self::PRIMARY_KEY.iter().map(|k| format!("{table}.{k} = p.{k}")).collect::<Vec<_>>().join(" AND ");
        let q = format!("UPDATE {table} SET ({set}) = ({p_set}) FROM json_populate_record(NULL::{table}, $1::json) p WHERE {where_clause} RETURNING {table}.*");
        sqlx::query_as::<_, Self>(&q).bind(sqlx::types::Json(self)).fetch_one(pool).await
    }

    async fn insert(&self, pool: &sqlx::PgPool) -> Result<Self, sqlx::Error> where Self: Sized {
        let table = Self::TABLE_NAME;
        let q = format!("INSERT INTO {table} SELECT * FROM json_populate_record(NULL::{table}, $1::json) RETURNING *");
        sqlx::query_as::<_, Self>(&q).bind(sqlx::types::Json(self)).fetch_one(pool).await
    }
}

#[cfg(feature = "postgres")]
impl<T> TableEntity for T where T: Table + Serialize + for<'r> sqlx::FromRow<'r, sqlx::postgres::PgRow> + Send + Unpin {}
