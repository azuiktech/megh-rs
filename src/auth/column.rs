//! The Rust-to-database type mapping. A new type needs one `impl Column` (or `text_column!`, or `#[derive(Column)]` for an
//! enum) and nothing else.

pub use chrono::{DateTime, Utc};
use chrono::{NaiveDate, NaiveTime};
use rust_decimal::Decimal;
use sea_query::{ArrayType, Value, ValueType};
use serde::Serialize;
use serde::de::DeserializeOwned;
use uuid::Uuid;

/// What a list column holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Elem {
    Text,
    Uuid,
    Int,
}

impl Elem {
    pub fn array_type(self) -> ArrayType {
        match self {
            Elem::Text => ArrayType::String,
            Elem::Uuid => ArrayType::Uuid,
            Elem::Int => ArrayType::BigInt,
        }
    }
}

/// The database primitives a column holds; rows are read and written by it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Text,
    Uuid,
    Date,
    Time,
    Timestamp,
    Bool,
    Int,
    Float,
    Decimal,
    Bytes,
    Json,
    Array(Elem),
}

impl Kind {
    /// The element of a list, as a constant: a list of anything but text, uuids or integers does not compile.
    pub const fn elem(self) -> Elem {
        match self {
            Kind::Text => Elem::Text,
            Kind::Uuid => Elem::Uuid,
            Kind::Int => Elem::Int,
            _ => panic!("a list holds text, uuids or integers"),
        }
    }

    /// The null of this kind, the one `Column` for `Option` and the row reader agree on.
    pub fn null(self) -> Value {
        match self {
            Kind::Text => Value::String(None),
            Kind::Uuid => Value::Uuid(None),
            Kind::Date => Value::ChronoDate(None),
            Kind::Time => Value::ChronoTime(None),
            Kind::Timestamp => Value::ChronoDateTimeUtc(None),
            Kind::Bool => Value::Bool(None),
            Kind::Int => Value::BigInt(None),
            Kind::Float => Value::Double(None),
            Kind::Decimal => Value::Decimal(None),
            Kind::Bytes => Value::Bytes(None),
            Kind::Json => Value::Json(None),
            Kind::Array(elem) => Value::Array(elem.array_type(), None),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ColumnError {
    #[error("no value for column {0}")]
    Missing(&'static str),
    #[error("{0}")]
    Invalid(String),
}

pub trait Column: Sized {
    const KIND: Kind;
    /// The database's own type of the column when it is not what `KIND` says, such as a PostgreSQL enum; values are
    /// written to it and read from it as text.
    const TYPE_NAME: Option<&'static str> = None;
    fn to_value(&self) -> Value;
    fn from_value(value: Value) -> Result<Self, ColumnError>;
}

macro_rules! column {
    ($($ty:ty => $kind:ident),* $(,)?) => {$(
        impl Column for $ty {
            const KIND: Kind = Kind::$kind;
            fn to_value(&self) -> Value {
                Value::from(self.clone())
            }
            fn from_value(value: Value) -> Result<Self, ColumnError> {
                <$ty as ValueType>::try_from(value).map_err(|_| ColumnError::Invalid(concat!("not a ", stringify!($ty)).into()))
            }
        }
    )*};
}

column!(
    String => Text, bool => Bool, i64 => Int, f64 => Float, Decimal => Decimal, Uuid => Uuid, NaiveDate => Date,
    NaiveTime => Time, DateTime<Utc> => Timestamp, Vec<u8> => Bytes,
);

impl Column for i32 {
    const KIND: Kind = Kind::Int;
    fn to_value(&self) -> Value {
        i64::from(*self).into()
    }
    fn from_value(value: Value) -> Result<Self, ColumnError> {
        <i32 as TryFrom<i64>>::try_from(i64::from_value(value)?).map_err(|error| ColumnError::Invalid(error.to_string()))
    }
}

impl<T: Column> Column for Option<T> {
    const KIND: Kind = T::KIND;
    const TYPE_NAME: Option<&'static str> = T::TYPE_NAME;
    fn to_value(&self) -> Value {
        self.as_ref().map_or_else(|| T::KIND.null(), T::to_value)
    }
    fn from_value(value: Value) -> Result<Self, ColumnError> {
        (value != T::KIND.null()).then(|| T::from_value(value)).transpose()
    }
}

/// A list column: a native array where the database has them, JSON text where it does not.
impl<T: Column> Column for Vec<T> {
    const KIND: Kind = Kind::Array(T::KIND.elem());
    fn to_value(&self) -> Value {
        Value::Array(T::KIND.elem().array_type(), Some(Box::new(self.iter().map(T::to_value).collect())))
    }
    fn from_value(value: Value) -> Result<Self, ColumnError> {
        match value {
            Value::Array(_, Some(items)) => items.into_iter().map(T::from_value).collect(),
            _ => Err(ColumnError::Invalid("not a list".into())),
        }
    }
}

/// `Column` for newtypes over text that are built with `TryFrom<String>` and read with `AsRef<str>`.
#[macro_export]
macro_rules! text_column {
    ($($ty:ty),* $(,)?) => {$(
        impl $crate::auth::column::Column for $ty {
            const KIND: $crate::auth::column::Kind = $crate::auth::column::Kind::Text;
            fn to_value(&self) -> $crate::sea_query::Value {
                AsRef::<str>::as_ref(self).to_string().into()
            }
            fn from_value(value: $crate::sea_query::Value) -> Result<Self, $crate::auth::column::ColumnError> {
                <$ty>::try_from(<String as $crate::auth::column::Column>::from_value(value)?)
                    .map_err(|error| $crate::auth::column::ColumnError::Invalid(error.to_string()))
            }
        }
    )*};
}

/// `Column` for newtypes over a type that is a `Column`, built with `TryFrom` and read with `Into` of that type
/// (a nutype with `TryFrom` or `From`, and `Into`): `newtype_column!(Micros: i64, Percent: Decimal)`.
#[macro_export]
macro_rules! newtype_column {
    ($($ty:ty: $inner:ty),* $(,)?) => {$(
        impl $crate::auth::column::Column for $ty {
            const KIND: $crate::auth::column::Kind = <$inner as $crate::auth::column::Column>::KIND;
            fn to_value(&self) -> $crate::sea_query::Value {
                <$inner as $crate::auth::column::Column>::to_value(&<$inner>::from(self.clone()))
            }
            #[allow(unreachable_code)]
            fn from_value(value: $crate::sea_query::Value) -> Result<Self, $crate::auth::column::ColumnError> {
                <$ty>::try_from(<$inner as $crate::auth::column::Column>::from_value(value)?)
                    .map_err(|error| $crate::auth::column::ColumnError::Invalid(error.to_string()))
            }
        }
    )*};
}

/// A JSON column: the one value is converted with serde, the rest of the entity is not.
pub fn json_value<T: Serialize>(value: &T) -> Value {
    match serde_json::to_value(value) {
        Ok(serde_json::Value::Null) | Err(_) => Value::Json(None),
        Ok(json) => Value::Json(Some(Box::new(json))),
    }
}

pub fn from_json_value<T: DeserializeOwned>(value: Value) -> Result<T, ColumnError> {
    let json = match value {
        Value::Json(json) => json.map_or(serde_json::Value::Null, |json| *json),
        _ => return Err(ColumnError::Invalid("not JSON".into())),
    };
    serde_json::from_value(json).map_err(|error| ColumnError::Invalid(error.to_string()))
}

/// A JSON column from the JSON of a patch or filter; `null` is the column's NULL.
pub fn json_column(json: &serde_json::Value) -> Value {
    match json {
        serde_json::Value::Null => Value::Json(None),
        other => Value::Json(Some(Box::new(other.clone()))),
    }
}

/// The database value of a column from JSON, converted by the field's own type (its validation and its encoding).
pub fn from_json<T: Column + DeserializeOwned>(json: &serde_json::Value) -> Result<Value, ColumnError> {
    serde_json::from_value::<T>(json.clone()).map(|value| value.to_value()).map_err(|error| ColumnError::Invalid(error.to_string()))
}

pub fn now() -> DateTime<Utc> {
    Utc::now()
}
