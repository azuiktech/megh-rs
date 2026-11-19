//! `#[derive(Column)]` for an enum: stored as text, named as sqlx names it (`#[sqlx(rename_all = "...")]`, `#[sqlx(rename = "...")]`
//! on a variant, `#[sqlx(type_name = "...")]` for a database enum type).

use proc_macro2::TokenStream;
use quote::quote;
use syn::{Data, DeriveInput, Fields, LitStr};

use crate::parse::{apply_rename_all, crate_path, sqlx_attrs};

/// The `rename_all`, `type_name` and `rename` of the sqlx attributes among `attrs`.
fn sqlx_options(attrs: &[syn::Attribute]) -> syn::Result<(Option<String>, Option<String>, Option<String>)> {
    let mut found = (None, None, None);
    sqlx_attrs(attrs).iter().try_for_each(|attr| {
        attr.parse_nested_meta(|meta| {
            let text = |meta: &syn::meta::ParseNestedMeta| meta.value()?.parse::<LitStr>().map(|lit| lit.value());
            match meta.path.get_ident().map(|ident| ident.to_string()).as_deref() {
                Some("rename_all") => found.0 = Some(text(&meta)?),
                Some("type_name") => found.1 = Some(text(&meta)?),
                Some("rename") => found.2 = Some(text(&meta)?),
                _ if meta.input.peek(syn::Token![=]) => drop(meta.value()?.parse::<syn::Expr>()?),
                _ => {}
            }
            Ok(())
        })
    })?;
    Ok(found)
}

pub fn expand(input: DeriveInput) -> syn::Result<TokenStream> {
    let krate = crate_path();
    let Data::Enum(data) = &input.data else {
        return Err(syn::Error::new_spanned(&input, "Column is derived for enums; a newtype uses text_column! or int_column!"));
    };
    let (rename_all, type_name, _) = sqlx_options(&input.attrs)?;
    let type_name = type_name.filter(|name| name != "text");
    let names = data
        .variants
        .iter()
        .map(|variant| {
            let ident = &variant.ident;
            let name = sqlx_options(&variant.attrs)?.2.unwrap_or_else(|| apply_rename_all(&ident.to_string(), rename_all.as_deref()));
            match variant.fields {
                Fields::Unit => Ok((ident, name)),
                _ => Err(syn::Error::new_spanned(variant, "only variants without fields")),
            }
        })
        .collect::<syn::Result<Vec<_>>>()?;
    let ident = &input.ident;
    let (variants, texts): (Vec<_>, Vec<_>) = names.into_iter().unzip();
    let type_name = match type_name {
        Some(name) => quote!(Some(#name)),
        None => quote!(None),
    };
    Ok(quote! {
        #[automatically_derived]
        impl #krate::auth::column::Column for #ident {
            const KIND: #krate::auth::column::Kind = #krate::auth::column::Kind::Text;
            const TYPE_NAME: Option<&'static str> = #type_name;

            fn to_value(&self) -> #krate::sea_query::Value {
                match self {
                    #(Self::#variants => #texts,)*
                }
                .into()
            }

            fn from_value(value: #krate::sea_query::Value) -> Result<Self, #krate::auth::column::ColumnError> {
                match <String as #krate::auth::column::Column>::from_value(value)?.as_str() {
                    #(#texts => Ok(Self::#variants),)*
                    other => Err(#krate::auth::column::ColumnError::Invalid(format!("{other} is not a {}", stringify!(#ident)))),
                }
            }
        }
    })
}
