//! What `#[entity]` generates for a struct: its table, its columns, and how each field is bound and read through its
//! own `sqlx::Type`.


pub use chrono::{DateTime, Utc};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::auth::audit::AuditPolicy;
use crate::auth::db::{Param, Record, Value};

#[derive(Debug, thiserror::Error)]
pub enum ColumnError {
    #[error("unknown column {0}")]
    Unknown(String),
    #[error("{0}")]
    Invalid(String),
}

/// A struct mapped to a table by `#[entity]`.
pub trait Table: Record + Sized + 'static {
    const TABLE_NAME: &'static str;
    const PRIMARY_KEY: &'static [&'static str];
    const COLUMNS: &'static [&'static str];
    /// The columns stored as an `Array`.
    const LISTS: &'static [&'static str];
    /// Whether delete marks `deleted_at` (and reads skip marked rows) instead of removing the row.
    const SOFT_DELETE: bool;
    const AUDIT: AuditPolicy;

    /// The fields' values, in the order of `COLUMNS`.
    fn params(self) -> Vec<Value>;

    /// The value of `column` from JSON, as the field's own type takes it.
    fn param(column: &str, json: &serde_json::Value) -> Result<Value, ColumnError>;
}

/// The time a row is made, for an entity deserialized without one.
pub fn now() -> DateTime<Utc> {
    Utc::now()
}

/// `json` as the value of a field of type `T`.
pub fn param<T: DeserializeOwned + Param<'static>>(json: &serde_json::Value) -> Result<Value, ColumnError> {
    serde_json::from_value::<T>(json.clone()).map(|value| Box::new(value) as Value).map_err(|error| ColumnError::Invalid(error.to_string()))
}

/// `json` as the value of a field of type `T` stored as JSON (`#[sqlx(json)]`): it must be a `T`.
pub fn param_json<T: DeserializeOwned + Serialize + Send + 'static>(json: &serde_json::Value) -> Result<Value, ColumnError> {
    serde_json::from_value::<T>(json.clone()).map(|value| Box::new(sqlx::types::Json(value)) as Value).map_err(|error| ColumnError::Invalid(error.to_string()))
}

/// `json` as the value of a field of type `F` stored as `S` (`#[sqlx(try_from = "S")]`).
pub fn param_from<F: DeserializeOwned, S: From<F> + Param<'static>>(json: &serde_json::Value) -> Result<Value, ColumnError> {
    serde_json::from_value::<F>(json.clone()).map(|value| Box::new(S::from(value)) as Value).map_err(|error| ColumnError::Invalid(error.to_string()))
}

/// A condition on rows: SQL with `?` placeholders (the text of each driver), and the values bound to them.
pub struct Where {
    pub(crate) standard: String,
    pub(crate) postgres: String,
    pub(crate) sqlite: String,
    pub(crate) params: Vec<Value>,
}

impl Where {
    /// A condition in standard SQL.
    pub fn new(text: impl Into<String>, params: Vec<Value>) -> Self {
        let text = text.into();
        Where { standard: text.clone(), postgres: text.clone(), sqlite: text, params }
    }

    /// The text on PostgreSQL, where its SQL differs from the standard.
    pub fn postgres(self, text: impl Into<String>) -> Self {
        Where { postgres: text.into(), ..self }
    }

    /// The text on SQLite, where its SQL differs from the standard.
    pub fn sqlite(self, text: impl Into<String>) -> Self {
        Where { sqlite: text.into(), ..self }
    }

    pub fn and(self, other: Where) -> Where {
        self.join("AND", other)
    }

    pub fn or(self, other: Where) -> Where {
        self.join("OR", other)
    }

    /// Every row.
    pub fn all() -> Where {
        Where::new("1 = 1", vec![])
    }

    /// Rows not matching.
    pub fn not(self) -> Where {
        let wrap = |text: String| format!("NOT ({text})");
        Where { standard: wrap(self.standard), postgres: wrap(self.postgres), sqlite: wrap(self.sqlite), params: self.params }
    }

    fn join(mut self, operator: &str, other: Where) -> Where {
        let join = |left: String, right: String| format!("({left}) {operator} ({right})");
        self.params.extend(other.params);
        Where {
            standard: join(self.standard, other.standard),
            postgres: join(self.postgres, other.postgres),
            sqlite: join(self.sqlite, other.sqlite),
            params: self.params,
        }
    }
}

/// A JSON document kept in a text column, as megh-go writes them. NULL and `null` read as the default.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct JsonText<T>(pub T);

/// Read with `#[sqlx(try_from = "Option<String>")]`.
impl<T: DeserializeOwned + Default> TryFrom<Option<String>> for JsonText<T> {
    type Error = serde_json::Error;

    fn try_from(text: Option<String>) -> Result<Self, Self::Error> {
        let json = text.map_or(Ok(None), |text| serde_json::from_str::<Option<T>>(&text))?;
        Ok(Self(json.unwrap_or_default()))
    }
}

/// Written with `#[sqlx(try_from = "Option<String>")]`.
impl<T: Serialize> From<JsonText<T>> for Option<String> {
    fn from(json: JsonText<T>) -> Self {
        serde_json::to_string(&json.0).ok()
    }
}

/// Stored as the text of its JSON on every driver.
macro_rules! json_text {
    ($db:ty) => {
        impl<T> sqlx::Type<$db> for JsonText<T> {
            fn type_info() -> <$db as sqlx::Database>::TypeInfo {
                <String as sqlx::Type<$db>>::type_info()
            }
            fn compatible(ty: &<$db as sqlx::Database>::TypeInfo) -> bool {
                <String as sqlx::Type<$db>>::compatible(ty)
            }
        }

        impl<'q, T: Serialize> sqlx::Encode<'q, $db> for JsonText<T> {
            fn encode_by_ref(&self, buf: &mut <$db as sqlx::Database>::ArgumentBuffer<'q>) -> Result<sqlx::encode::IsNull, sqlx::error::BoxDynError> {
                <String as sqlx::Encode<'q, $db>>::encode(serde_json::to_string(&self.0)?, buf)
            }
        }

        impl<'r, T: DeserializeOwned + Default> sqlx::Decode<'r, $db> for JsonText<T> {
            fn decode(value: <$db as sqlx::Database>::ValueRef<'r>) -> Result<Self, sqlx::error::BoxDynError> {
                Ok(JsonText::try_from(Some(<String as sqlx::Decode<'r, $db>>::decode(value)?))?)
            }
        }
    };
}

#[cfg(feature = "postgres")]
json_text!(sqlx::Postgres);
#[cfg(feature = "sqlite")]
json_text!(sqlx::Sqlite);

/// A list stored as a PostgreSQL array, and as the text of a JSON array on SQLite: a `Vec<T>` field annotated
/// `#[sqlx(try_from = "Array<T>")]`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "axum", derive(utoipa::ToSchema))]
#[serde(transparent)]
pub struct Array<T>(pub Vec<T>);

impl<T> std::ops::Deref for Array<T> {
    type Target = Vec<T>;
    fn deref(&self) -> &Vec<T> {
        &self.0
    }
}

impl<T> From<Vec<T>> for Array<T> {
    fn from(items: Vec<T>) -> Self {
        Array(items)
    }
}

impl<T> From<Array<T>> for Vec<T> {
    fn from(items: Array<T>) -> Self {
        items.0
    }
}

impl<T> FromIterator<T> for Array<T> {
    fn from_iter<I: IntoIterator<Item = T>>(items: I) -> Self {
        Array(items.into_iter().collect())
    }
}

#[cfg(feature = "postgres")]
impl<T: sqlx::postgres::PgHasArrayType> sqlx::Type<sqlx::Postgres> for Array<T> {
    fn type_info() -> sqlx::postgres::PgTypeInfo {
        T::array_type_info()
    }
    fn compatible(ty: &sqlx::postgres::PgTypeInfo) -> bool {
        T::array_compatible(ty)
    }
}

#[cfg(feature = "postgres")]
impl<'q, T> sqlx::Encode<'q, sqlx::Postgres> for Array<T>
where
    T: for<'a> sqlx::Encode<'a, sqlx::Postgres> + sqlx::Type<sqlx::Postgres> + sqlx::postgres::PgHasArrayType,
{
    fn encode_by_ref(&self, buf: &mut sqlx::postgres::PgArgumentBuffer) -> Result<sqlx::encode::IsNull, sqlx::error::BoxDynError> {
        <&[T] as sqlx::Encode<'q, sqlx::Postgres>>::encode(self.0.as_slice(), buf)
    }
}

#[cfg(feature = "postgres")]
impl<'r, T> sqlx::Decode<'r, sqlx::Postgres> for Array<T>
where
    T: for<'a> sqlx::Decode<'a, sqlx::Postgres> + sqlx::Type<sqlx::Postgres> + sqlx::postgres::PgHasArrayType,
{
    fn decode(value: sqlx::postgres::PgValueRef<'r>) -> Result<Self, sqlx::error::BoxDynError> {
        <Vec<T> as sqlx::Decode<'r, sqlx::Postgres>>::decode(value).map(Array)
    }
}

#[cfg(feature = "sqlite")]
impl<T> sqlx::Type<sqlx::Sqlite> for Array<T> {
    fn type_info() -> sqlx::sqlite::SqliteTypeInfo {
        <String as sqlx::Type<sqlx::Sqlite>>::type_info()
    }
    fn compatible(ty: &sqlx::sqlite::SqliteTypeInfo) -> bool {
        <String as sqlx::Type<sqlx::Sqlite>>::compatible(ty)
    }
}

#[cfg(feature = "sqlite")]
impl<'q, T: Serialize> sqlx::Encode<'q, sqlx::Sqlite> for Array<T> {
    fn encode_by_ref(&self, buf: &mut Vec<sqlx::sqlite::SqliteArgumentValue<'q>>) -> Result<sqlx::encode::IsNull, sqlx::error::BoxDynError> {
        <String as sqlx::Encode<'q, sqlx::Sqlite>>::encode(serde_json::to_string(&self.0)?, buf)
    }
}

#[cfg(feature = "sqlite")]
impl<'r, T: DeserializeOwned> sqlx::Decode<'r, sqlx::Sqlite> for Array<T> {
    fn decode(value: sqlx::sqlite::SqliteValueRef<'r>) -> Result<Self, sqlx::error::BoxDynError> {
        Ok(Array(serde_json::from_str(&<String as sqlx::Decode<'r, sqlx::Sqlite>>::decode(value)?)?))
    }
}
