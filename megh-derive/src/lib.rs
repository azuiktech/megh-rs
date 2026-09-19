//! Procedural macro crate providing #[derive(Table)] for megh.

mod codegen;
mod parse;

use proc_macro::TokenStream;
use syn::{parse_macro_input, DeriveInput};

#[proc_macro_derive(Table, attributes(table, sqlx, key))]
pub fn derive_table(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    match parse::TableConfig::from_input(&input) {
        Ok(config) => codegen::generate_table_impl(&input.ident, &config).into(),
        Err(err) => err.to_compile_error().into(),
    }
}
