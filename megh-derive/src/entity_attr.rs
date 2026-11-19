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
        let path = &meta.path;
        if path.is_ident("table") {
            options.table = Some(meta.value()?.parse::<LitStr>()?.value());
        } else if path.is_ident("keys") {
            let value = meta.value()?;
            let content;
            syn::bracketed!(content in value);
            let keys = Punctuated::<LitStr, Token![,]>::parse_terminated(&content)?;
            options.keys = Some(keys.into_iter().map(|key| key.value()).collect());
        } else if path.is_ident("timestamps") {
            options.timestamps = true;
        } else if path.is_ident("soft_delete") {
            options.soft_delete = true;
        } else if path.is_ident("audit") {
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
        } else {
            return Err(meta.error("unknown option"));
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
    let skipped: Vec<_> = parsed.iter().filter(|(_, column)| column.is_none()).map(|(field, _)| field.ident.clone().unwrap()).collect();

    let table = options.table.clone().unwrap_or_else(|| item.ident.to_string().to_snake_case());
    let keys = options.keys.clone().unwrap_or_else(|| vec!["id".to_string()]);
    let soft_delete = options.soft_delete;
    let audit = options.audit.clone().unwrap_or(quote!(Disabled));
    let ident = &item.ident;
    let (flat_ident, flat_ty): (Vec<_>, Vec<_>) = flattened.iter().cloned().unzip();

    let meta = columns.iter().map(|column| {
        let name = &column.name;
        let primary = keys.contains(name);
        let (kind, type_name) = match column.is_json {
            true => (quote!(#krate::auth::column::Kind::Json), quote!(None)),
            false => {
                let ty = &column.ty;
                (quote!(<#ty as #krate::auth::column::Column>::KIND), quote!(<#ty as #krate::auth::column::Column>::TYPE_NAME))
            }
        };
        quote!(#krate::ColumnMeta { name: #name, kind: #kind, type_name: #type_name, is_primary_key: #primary })
    });
    let own = quote!(&[#(#meta),*]);
    let all = match flattened.is_empty() {
        true => own,
        false => quote!(#krate::__constcat::concat_slices!([#krate::ColumnMeta]: #own, #(<#flat_ty as #krate::Table>::COLUMNS),*)),
    };

    let to_value = columns.iter().map(|column| {
        let (name, field, ty) = (&column.name, &column.ident, &column.ty);
        match column.is_json {
            true => quote!((#name, #krate::auth::column::json_value(&self.#field))),
            false => quote!((#name, <#ty as #krate::auth::column::Column>::to_value(&self.#field))),
        }
    });
    let from_value = columns.iter().map(|column| {
        let (name, field, ty) = (&column.name, &column.ident, &column.ty);
        let next = quote!(row.next().ok_or(#krate::auth::column::ColumnError::Missing(#name))?);
        match column.is_json {
            true => quote!(#field: #krate::auth::column::from_json_value(#next)?),
            false => quote!(#field: <#ty as #krate::auth::column::Column>::from_value(#next)?),
        }
    });
    let from_json = columns.iter().map(|column| {
        let (name, ty) = (&column.name, &column.ty);
        match column.is_json {
            true => quote!(#name => Ok(#krate::auth::column::json_column(json))),
            false => quote!(#name => #krate::auth::column::from_json::<#ty>(json)),
        }
    });

    let table_impl = quote! {
        #[automatically_derived]
        impl #krate::Table for #ident {
            const TABLE_NAME: &'static str = #table;
            const PRIMARY_KEY: &'static [&'static str] = &[#(#keys),*];
            const COLUMNS: &'static [#krate::ColumnMeta] = #all;
            const SOFT_DELETE: bool = #soft_delete;
            const AUDIT: #krate::auth::audit::AuditPolicy = #krate::auth::audit::AuditPolicy::#audit;

            fn values(&self) -> Vec<(&'static str, #krate::sea_query::Value)> {
                let mut values = vec![#(#to_value),*];
                #(values.extend(<#flat_ty as #krate::Table>::values(&self.#flat_ident));)*
                values
            }

            fn from_values(row: Vec<#krate::sea_query::Value>) -> Result<Self, #krate::auth::column::ColumnError> {
                let mut row = row.into_iter();
                Ok(Self {
                    #(#from_value,)*
                    #(#flat_ident: <#flat_ty as #krate::Table>::from_values(row.by_ref().take(<#flat_ty as #krate::Table>::COLUMNS.len()).collect())?,)*
                    #(#skipped: ::core::default::Default::default(),)*
                })
            }

            fn from_json(column: &str, json: &serde_json::Value) -> Result<#krate::sea_query::Value, #krate::auth::column::ColumnError> {
                match column {
                    #(#from_json,)*
                    other => {
                        #(if <#flat_ty as #krate::Table>::COLUMNS.iter().any(|column| column.name == other) {
                            return <#flat_ty as #krate::Table>::from_json(other, json);
                        })*
                        Err(#krate::auth::column::ColumnError::Invalid(format!("unknown column {other}")))
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
    strip(&mut item, attributes.contains("FromRow"));
    Ok(quote!(#item #table_impl #response))
}

/// `created_at`, `updated_at` and `deleted_at` as the options ask for them. A value a caller gives them is replaced
/// when the row is written; deserializing an entity without them fills them in.
fn inject(item: &mut ItemStruct, options: &Options, krate: &TokenStream, schema: bool) -> syn::Result<()> {
    let Fields::Named(named) = &mut item.fields else { return Ok(()) };
    let now = quote!(#krate::auth::column::now).to_string();
    let time = quote!(#krate::auth::column::DateTime<#krate::auth::column::Utc>);
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

/// The sqlx attributes only describe columns to this macro; they are removed unless sqlx's own derive reads them.
fn strip(item: &mut ItemStruct, keeps_sqlx: bool) {
    let is_ours = |attr: &syn::Attribute| attr.path().is_ident("table") || (!keeps_sqlx && (attr.path().is_ident("sqlx") || attr.path().is_ident("key")));
    item.attrs.retain(|attr| !is_ours(attr));
    if let Fields::Named(named) = &mut item.fields {
        named.named.iter_mut().for_each(|field| field.attrs.retain(|attr| !is_ours(attr)));
    }
}
