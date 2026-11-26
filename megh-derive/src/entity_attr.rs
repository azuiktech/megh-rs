//! `#[entity(...)]`: the table of a struct, the timestamp fields it injects, and the mapping of its fields to column values.

use heck::ToSnakeCase;
use proc_macro2::TokenStream;
use quote::quote;
use syn::punctuated::Punctuated;
use syn::{Field, Fields, ItemStruct, LitStr, Token};

use crate::parse::{crate_path, is_flattened, parse_field, rename_all as sqlx_rename_all};

#[derive(Default)]
struct Options {
    table: Option<String>,
    keys: Option<Vec<String>>,
    timestamps: bool,
    soft_delete: bool,
    audit: Option<TokenStream>,
}

fn options(args: TokenStream) -> syn::Result<Options> {
    let mut options = Options::default();
    let parser = syn::meta::parser(|meta| {
        let name = meta.path.get_ident().map(ToString::to_string);
        match name.as_deref() {
            Some("table") => options.table = Some(meta.value()?.parse::<LitStr>()?.value()),
            Some("keys") => {
                let value = meta.value()?;
                let content;
                syn::bracketed!(content in value);
                let keys = Punctuated::<LitStr, Token![,]>::parse_terminated(&content)?;
                options.keys = Some(keys.into_iter().map(|key| key.value()).collect());
            }
            Some("timestamps") => options.timestamps = true,
            Some("soft_delete") => options.soft_delete = true,
            Some("audit") => {
                let value = meta.value()?;
                options.audit = Some(match value.peek(LitStr) {
                    true => {
                        let table: LitStr = value.parse()?;
                        quote!(Dedicated(#table))
                    }
                    false => match value.parse::<syn::Ident>()?.to_string().as_str() {
                        "shared" => quote!(Shared),
                        "per_table" => quote!(PerTable),
                        _ => return Err(meta.error("audit is `shared`, `per_table` or a table name in quotes")),
                    },
                });
            }
            _ => return Err(meta.error("unknown option")),
        }
        Ok(())
    });
    syn::parse::Parser::parse2(parser, args)?;
    Ok(options)
}

pub fn expand(args: TokenStream, mut item: ItemStruct) -> syn::Result<TokenStream> {
    let options = options(args)?;
    let krate = crate_path();
    let attributes = item.attrs.iter().map(|attr| quote!(#attr).to_string()).collect::<Vec<_>>().join(" ");
    inject(&mut item, &options, &krate, attributes.contains("ToSchema"))?;
    let rename_all = sqlx_rename_all(&item.attrs)?;
    let Fields::Named(named) = &item.fields else {
        return Err(syn::Error::new_spanned(&item, "entity needs named fields"));
    };

    let flattened: Vec<_> = named.named.iter().filter(|field| is_flattened(field)).map(|field| (field.ident.clone().unwrap(), field.ty.clone())).collect();
    let parsed = named
        .named
        .iter()
        .filter(|field| !is_flattened(field))
        .map(|field| parse_field(field, rename_all.as_deref()).map(|column| (field, column)))
        .collect::<syn::Result<Vec<_>>>()?;
    let columns: Vec<_> = parsed.iter().filter_map(|(_, column)| column.as_ref()).collect();

    let table = options.table.clone().unwrap_or_else(|| item.ident.to_string().to_snake_case());
    let keys = options.keys.clone().unwrap_or_else(|| vec!["id".to_string()]);
    let soft_delete = options.soft_delete;
    let audit = options.audit.clone().unwrap_or(quote!(Disabled));
    let ident = &item.ident;
    let (flat_ident, flat_ty): (Vec<_>, Vec<_>) = flattened.iter().cloned().unzip();

    let names = columns.iter().map(|column| &column.name);
    let own = quote!(&[#(#names),*]);
    let all = concat(&krate, own, &flat_ty, quote!(COLUMNS));
    let lists = columns.iter().filter(|column| column.stored.as_ref().is_some_and(is_array)).map(|column| &column.name);
    let all_lists = concat(&krate, quote!(&[#(#lists),*]), &flat_ty, quote!(LISTS));

    let params = columns.iter().map(|column| {
        let field = &column.ident;
        match (column.is_json, &column.stored) {
            (true, _) => quote!(Box::new(#krate::__sqlx::types::Json(self.#field)) as #krate::auth::db::Value),
            (false, Some(stored)) => quote!(Box::new(<#stored as ::core::convert::From<_>>::from(self.#field)) as #krate::auth::db::Value),
            (false, None) => quote!(Box::new(self.#field) as #krate::auth::db::Value),
        }
    });
    let param = columns.iter().map(|column| {
        let (name, ty) = (&column.name, &column.ty);
        match (column.is_json, &column.stored) {
            (true, _) => quote!(#name => #krate::auth::entity::param_json::<#ty>(json)),
            (false, Some(stored)) => quote!(#name => #krate::auth::entity::param_from::<#ty, #stored>(json)),
            (false, None) => quote!(#name => #krate::auth::entity::param::<#ty>(json)),
        }
    });
    let table_impl = quote! {
        #[automatically_derived]
        impl #krate::Table for #ident {
            const TABLE_NAME: &'static str = #table;
            const PRIMARY_KEY: &'static [&'static str] = &[#(#keys),*];
            const COLUMNS: &'static [&'static str] = #all;
            const LISTS: &'static [&'static str] = #all_lists;
            const SOFT_DELETE: bool = #soft_delete;
            const AUDIT: #krate::auth::audit::AuditPolicy = #krate::auth::audit::AuditPolicy::#audit;

            fn params(self) -> Vec<#krate::auth::db::Value> {
                let mut params = vec![#(#params),*];
                #(params.extend(<#flat_ty as #krate::Table>::params(self.#flat_ident));)*
                params
            }

            fn param(column: &str, json: &serde_json::Value) -> Result<#krate::auth::db::Value, #krate::auth::entity::ColumnError> {
                match column {
                    #(#param,)*
                    other => {
                        #(if <#flat_ty as #krate::Table>::COLUMNS.iter().any(|name| *name == other) {
                            return <#flat_ty as #krate::Table>::param(other, json);
                        })*
                        Err(#krate::auth::entity::ColumnError::Unknown(other.to_string()))
                    }
                }
            }
        }
    };

    let response = (!keys.is_empty() && attributes.contains("Serialize")).then(|| {
        quote! {
            #[cfg(feature = "axum")]
            impl #krate::__axum::response::IntoResponse for #ident {
                fn into_response(self) -> #krate::__axum::response::Response {
                    #krate::__axum::response::IntoResponse::into_response(#krate::__axum::Json(self))
                }
            }
        }
    });
    strip(&mut item);
    let from_row = (!attributes.contains("FromRow")).then(|| quote!(#[derive(#krate::__sqlx::FromRow)]));
    Ok(quote!(#from_row #item #table_impl #response))
}

/// `created_at`, `updated_at` and `deleted_at` as the options ask for them. A value a caller gives them is replaced
/// when the row is written; deserializing an entity without them fills them in.
fn inject(item: &mut ItemStruct, options: &Options, krate: &TokenStream, schema: bool) -> syn::Result<()> {
    let Fields::Named(named) = &mut item.fields else { return Ok(()) };
    let now = quote!(#krate::auth::entity::now).to_string();
    let time = quote!(#krate::auth::entity::DateTime<#krate::auth::entity::Utc>);
    let read_only = schema.then(|| quote!(#[schema(read_only)]));
    let created = quote!(#read_only #[serde(default = #now)] pub created_at: #time);
    let updated = quote!(#read_only #[serde(default)] pub updated_at: Option<#time>);
    let deleted = quote!(#read_only #[serde(default)] pub deleted_at: Option<#time>);
    let wanted = [(options.timestamps, created), (options.timestamps, updated), (options.soft_delete, deleted)];
    wanted
        .into_iter()
        .filter(|(wanted, _)| *wanted)
        .map(|(_, field)| syn::parse::Parser::parse2(Field::parse_named, field))
        .try_for_each(|field| field.map(|field| named.named.push(field)))
}

/// `slice` followed by the `constant` of each flattened type.
fn concat(krate: &TokenStream, slice: TokenStream, flattened: &[syn::Type], constant: TokenStream) -> TokenStream {
    match flattened.is_empty() {
        true => slice,
        false => quote!(#krate::__constcat::concat_slices!([&'static str]: { const OWN: &[&str] = #slice; OWN }, #(<#flattened as #krate::Table>::#constant),*)),
    }
}

/// A column stored as megh's `Array`: `#[sqlx(try_from = "Array<T>")]`.
fn is_array(stored: &syn::Type) -> bool {
    match stored {
        syn::Type::Path(path) => path.path.segments.last().is_some_and(|segment| segment.ident == "Array"),
        _ => false,
    }
}

/// The attributes only this macro reads are removed; sqlx's own derive reads the `sqlx` ones.
fn strip(item: &mut ItemStruct) {
    let is_ours = |attr: &syn::Attribute| attr.path().is_ident("table") || attr.path().is_ident("key");
    item.attrs.retain(|attr| !is_ours(attr));
    match &mut item.fields {
        Fields::Named(named) => named.named.iter_mut().for_each(|field| field.attrs.retain(|attr| !is_ours(attr))),
        _ => {}
    }
}
