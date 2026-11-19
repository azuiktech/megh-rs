//! Procedural macros of megh: `#[entity]` maps a struct to a table, `#[derive(Column)]` maps an enum to a text column.

mod column_derive;
mod entity_attr;
mod parse;

use proc_macro::TokenStream;
use syn::{parse_macro_input, DeriveInput, ItemStruct};

/// Maps a struct to a table: `#[entity(table = "clients", keys = ["tenant_id", "id"], timestamps, soft_delete, audit = shared)]`.
#[proc_macro_attribute]
pub fn entity(args: TokenStream, input: TokenStream) -> TokenStream {
    let item = parse_macro_input!(input as ItemStruct);
    match entity_attr::expand(args.into(), item) {
        Ok(tokens) => tokens.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

/// Maps an enum to a text column, named as sqlx names it.
#[proc_macro_derive(Column, attributes(sqlx))]
pub fn derive_column(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    match column_derive::expand(input) {
        Ok(tokens) => tokens.into(),
        Err(err) => err.to_compile_error().into(),
    }
}
