//! What `#[entity]` generates for a struct: its table and the mapping of its fields to column values.

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::auth::audit::AuditPolicy;
use crate::auth::column::{ColumnError, Kind};

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

impl<T: Serialize + DeserializeOwned + Default> crate::auth::column::Column for JsonText<T> {
    const KIND: Kind = Kind::Text;
    fn to_value(&self) -> sea_query::Value {
        serde_json::to_string(&self.0).ok().into()
    }
    fn from_value(value: sea_query::Value) -> Result<Self, ColumnError> {
        let text = <Option<String> as crate::auth::column::Column>::from_value(value)?;
        Self::try_from(text).map_err(|error| ColumnError::Invalid(error.to_string()))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ColumnMeta {
    pub name: &'static str,
    pub kind: Kind,
    /// The database's own type when it is not plain text, such as a PostgreSQL enum.
    pub type_name: Option<&'static str>,
    pub is_primary_key: bool,
}

impl ColumnMeta {
    /// `value` as this column takes it: a database enum takes it cast to its type.
    pub fn expr(&self, value: sea_query::Value) -> sea_query::SimpleExpr {
        match self.type_name {
            Some(type_name) => sea_query::Expr::val(value).as_enum(sea_query::Alias::new(type_name)),
            None => sea_query::Expr::val(value).into(),
        }
    }
}

/// A struct mapped to a table by `#[entity]`.
pub trait Table: Sized {
    const TABLE_NAME: &'static str;
    const PRIMARY_KEY: &'static [&'static str];
    const COLUMNS: &'static [ColumnMeta];
    /// Whether delete marks `deleted_at` (and reads skip marked rows) instead of removing the row.
    const SOFT_DELETE: bool;
    const AUDIT: AuditPolicy;

    /// The columns' values, in the order of `COLUMNS`.
    fn values(&self) -> Vec<(&'static str, sea_query::Value)>;

    /// An entity from values in the order of `COLUMNS`.
    fn from_values(row: Vec<sea_query::Value>) -> Result<Self, ColumnError>;

    /// The database value of `column` from JSON, converted by the field's own type.
    fn from_json(column: &str, json: &serde_json::Value) -> Result<sea_query::Value, ColumnError>;
}
