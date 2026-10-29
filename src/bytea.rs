//! A `Vec<u8>` field of an entity kept in a `bytea` column: `#[serde(with = "megh::bytea")]`. Rows are written
//! through JSON, where Postgres reads a bytea as hex text (`\\x2550…`); without this the field is written as the
//! text of a JSON array. sqlx reads the column into the `Vec<u8>` as it is.

use serde::{Deserialize, Deserializer, Serializer};

pub fn serialize<S: Serializer>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(&format!("\\x{}", hex::encode(bytes)))
}

pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<u8>, D::Error> {
    let text = String::deserialize(deserializer)?;
    let digits = text.strip_prefix("\\x").ok_or_else(|| serde::de::Error::custom("bytea text starts with \\x"))?;
    hex::decode(digits).map_err(serde::de::Error::custom)
}
