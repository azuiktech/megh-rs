//! Code generation for Table trait implementation.

use proc_macro2::TokenStream;
use quote::quote;
use syn::Ident;

use crate::parse::TableConfig;

pub fn generate_table_impl(ident: &Ident, config: &TableConfig) -> TokenStream {
    let crate_path = resolve_crate_path();
    let table_name = &config.table_name;
    let pks = &config.primary_keys;

    let col_defs = config.columns.iter().map(|col| {
        let name = &col.name;
        let is_pk = col.is_primary_key;
        let is_json = col.is_json;
        let has_def = col.has_default;
        quote! {
            #crate_path::ColumnMeta {
                name: #name,
                is_primary_key: #is_pk,
                is_json: #is_json,
                has_default: #has_def,
            }
        }
    });

    quote! {
        #[automatically_derived]
        impl #crate_path::Table for #ident {
            const TABLE_NAME: &'static str = #table_name;
            const PRIMARY_KEY: &'static [&'static str] = &[#(#pks),*];
            const COLUMNS: &'static [#crate_path::ColumnMeta] = &[
                #(#col_defs),*
            ];
        }
    }
}

fn resolve_crate_path() -> TokenStream {
    match std::env::var("CARGO_PKG_NAME").as_deref() {
        Ok("megh") => quote!(crate),
        _ => quote!(::megh),
    }
}
