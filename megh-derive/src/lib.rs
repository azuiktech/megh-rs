//! Procedural macros of megh: `#[entity]` maps a struct to a table, `sqlx_type!` stores a type per
//! database.

mod entity_attr;
mod parse;
mod sqlx_type;

use proc_macro::TokenStream;
use syn::{parse_macro_input, ItemStruct};

/// Maps a struct to a table: `#[entity(table = "clients", keys = ["tenant_id", "id"], timestamps, soft_delete, audit = shared)]`.
#[proc_macro_attribute]
pub fn entity(args: TokenStream, input: TokenStream) -> TokenStream {
    let item = parse_macro_input!(input as ItemStruct);
    match entity_attr::expand(args.into(), item) {
        Ok(tokens) => tokens.into(),
        Err(err) => err.to_compile_error().into(),
    }
}


/// Stores a type per database: `sqlx_type!(T, match driver { Postgres => (write, read), _ => (write, read) })`.
#[proc_macro]
pub fn sqlx_type(input: TokenStream) -> TokenStream {
    let spec = parse_macro_input!(input as sqlx_type::Spec);
    sqlx_type::expand(spec).unwrap_or_else(syn::Error::into_compile_error).into()
}
