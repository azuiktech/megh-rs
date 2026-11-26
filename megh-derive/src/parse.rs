//! Reading the struct a macro is applied to: its sqlx annotations name the columns.

use heck::{ToKebabCase, ToLowerCamelCase, ToShoutySnakeCase, ToSnakeCase, ToUpperCamelCase};
use proc_macro2::TokenStream;
use quote::quote;
use syn::{Attribute, Field, LitStr};

pub struct ParsedColumn {
    pub ident: syn::Ident,
    pub ty: syn::Type,
    pub name: String,
    pub is_json: bool,
    /// `#[sqlx(try_from = "Stored")]`: the type the column is read as, and written as through `From`.
    pub stored: Option<syn::Type>,
}

/// The `sqlx(...)` attributes among `attrs`, also those written `cfg_attr(condition, sqlx(...))`, which a macro on the
/// item sees unexpanded.
pub fn sqlx_attrs(attrs: &[Attribute]) -> Vec<Attribute> {
    attrs
        .iter()
        .flat_map(|attr| match attr.path().is_ident("cfg_attr") {
            true => attr
                .parse_args_with(syn::punctuated::Punctuated::<syn::Meta, syn::Token![,]>::parse_terminated)
                .map(|metas| metas.into_iter().skip(1).filter(|meta| meta.path().is_ident("sqlx")).map(|meta| syn::parse_quote!(#[#meta])).collect())
                .unwrap_or_default(),
            false if attr.path().is_ident("sqlx") => vec![attr.clone()],
            false => vec![],
        })
        .collect()
}

/// `#[sqlx(rename_all = "...")]` of the struct.
pub fn rename_all(attrs: &[Attribute]) -> syn::Result<Option<String>> {
    let mut found = None;
    sqlx_attrs(attrs).iter().try_for_each(|attr| {
        attr.parse_nested_meta(|meta| {
            match meta.path.is_ident("rename_all") {
                true => found = Some(meta.value()?.parse::<LitStr>()?.value()),
                false if meta.input.peek(syn::Token![=]) => drop(meta.value()?.parse::<syn::Expr>()?),
                false => {}
            }
            Ok(())
        })
    })?;
    Ok(found)
}

pub fn is_flattened(field: &Field) -> bool {
    sqlx_attrs(&field.attrs).iter().any(|attr| {
        let mut flatten = false;
        let _ = attr.parse_nested_meta(|meta| {
            flatten |= meta.path.is_ident("flatten");
            if meta.input.peek(syn::Token![=]) {
                meta.value()?.parse::<syn::Expr>()?;
            }
            Ok(())
        });
        flatten
    })
}

/// The column of a field; `None` for a field with `#[sqlx(skip)]`.
pub fn parse_field(field: &Field, rename_all: Option<&str>) -> syn::Result<Option<ParsedColumn>> {
    let ident = field.ident.clone().unwrap();
    let (mut name, mut skip, mut is_json, mut stored) = (None, false, false, None);
    sqlx_attrs(&field.attrs).iter().try_for_each(|attr| {
        attr.parse_nested_meta(|meta| {
            match meta.path.get_ident().map(|ident| ident.to_string()).as_deref() {
                Some("skip") => skip = true,
                Some("rename") => name = Some(meta.value()?.parse::<LitStr>()?.value()),
                Some("try_from") => stored = Some(meta.value()?.parse::<LitStr>()?.parse::<syn::Type>()?),
                Some("json") => {
                    is_json = true;
                    if meta.input.peek(syn::token::Paren) {
                        meta.parse_nested_meta(|_| Ok(()))?;
                    }
                }
                _ if meta.input.peek(syn::Token![=]) => drop(meta.value()?.parse::<syn::Expr>()?),
                _ => {}
            }
            Ok(())
        })
    })?;
    let name = name.unwrap_or_else(|| apply_rename_all(&ident.to_string(), rename_all));
    Ok((!skip).then(|| ParsedColumn { ident, ty: field.ty.clone(), name, is_json, stored }))
}

/// The name of the crate the generated code refers to: `crate` inside megh, `::megh` outside.
pub fn crate_path() -> TokenStream {
    match std::env::var("CARGO_CRATE_NAME").as_deref() {
        Ok("megh") => quote!(crate),
        _ => quote!(::megh),
    }
}

/// A name written as sqlx's `rename_all` rule says.
pub fn apply_rename_all(name: &str, rename_all: Option<&str>) -> String {
    match rename_all {
        Some("lowercase") => name.to_lowercase(),
        Some("UPPERCASE") => name.to_uppercase(),
        Some("PascalCase") => name.to_upper_camel_case(),
        Some("camelCase") => name.to_lower_camel_case(),
        Some("snake_case") => name.to_snake_case(),
        Some("SCREAMING_SNAKE_CASE") => name.to_shouty_snake_case(),
        Some("kebab-case") => name.to_kebab_case(),
        _ => name.to_string(),
    }
}
