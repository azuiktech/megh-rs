//! `sqlx_type!`: a type stored per database through a write and a read closure, expanded to sqlx's `Type`, `Encode`
//! and `Decode`.

use proc_macro2::{Span, TokenStream};
use quote::{format_ident, quote};
use syn::parse::{Parse, ParseStream};
use syn::{Arm, Expr, ExprClosure, ExprMatch, Ident, Pat, ReturnType, Token, Type};

const DRIVERS: [&str; 2] = ["Postgres", "Sqlite"];

pub struct Spec {
    ty: Ident,
    arms: ExprMatch,
}

impl Parse for Spec {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let ty = input.parse()?;
        input.parse::<Token![,]>()?;
        let arms = input.parse()?;
        input.parse::<Option<Token![,]>>()?;
        Ok(Spec { ty, arms })
    }
}

struct Mapping {
    stored: Type,
    write: ExprClosure,
    read: ExprClosure,
}

pub fn expand(spec: Spec) -> syn::Result<TokenStream> {
    let arms = spec.arms.arms.iter().map(|arm| Ok((driver(&arm.pat)?, mapping(arm)?))).collect::<syn::Result<Vec<_>>>()?;
    let arm_for = |name: &str| {
        arms.iter()
            .find(|(driver, _)| driver.as_deref() == Some(name))
            .or_else(|| arms.iter().find(|(driver, _)| driver.is_none()))
    };
    Ok(DRIVERS.iter().filter_map(|name| arm_for(name).map(|(_, mapping)| impls(&spec.ty, name, mapping))).collect())
}

/// The driver an arm names, `None` for `_`.
fn driver(pat: &Pat) -> syn::Result<Option<String>> {
    match pat {
        Pat::Wild(_) => Ok(None),
        Pat::Ident(pat) if DRIVERS.contains(&pat.ident.to_string().as_str()) => Ok(Some(pat.ident.to_string())),
        other => Err(syn::Error::new_spanned(other, format!("expected one of {DRIVERS:?} or `_`"))),
    }
}

fn mapping(arm: &Arm) -> syn::Result<Mapping> {
    let pair = match &*arm.body {
        Expr::Tuple(pair) if pair.elems.len() == 2 => Ok(pair),
        other => Err(syn::Error::new_spanned(other, "expected `(write, read)`")),
    }?;
    let (write, read) = (closure(&pair.elems[0])?, closure(&pair.elems[1])?);
    let stored = match &write.output {
        ReturnType::Type(_, stored) => Ok((**stored).clone()),
        ReturnType::Default => Err(syn::Error::new_spanned(&write, "the write closure names its stored type: `-> Stored`")),
    }?;
    Ok(Mapping { stored, write, read })
}

/// A closure body as a function body.
fn block(body: &Expr) -> TokenStream {
    match body {
        Expr::Block(body) => quote!(#body),
        body => quote!({ #body }),
    }
}

fn closure(expr: &Expr) -> syn::Result<ExprClosure> {
    match expr {
        Expr::Closure(closure) => Ok(closure.clone()),
        other => Err(syn::Error::new_spanned(other, "expected a closure")),
    }
}

/// `Type`, `Encode` and `Decode` of `ty` for one driver, kept only when megh has that driver's feature.
fn impls(ty: &Ident, driver: &str, mapping: &Mapping) -> TokenStream {
    let gate = format_ident!("__if_{}", driver.to_lowercase());
    let db = Ident::new(driver, Span::call_site());
    let (type_impl, encode_impl, decode_impl) =
        (type_impl(ty, &db, mapping), encode_impl(ty, &db, mapping), decode_impl(ty, &db, mapping));
    quote! { ::megh::#gate! { #type_impl #encode_impl #decode_impl } }
}

fn type_impl(ty: &Ident, db: &Ident, Mapping { stored, .. }: &Mapping) -> TokenStream {
    quote! {
        impl ::megh::__sqlx::Type<::megh::__sqlx::#db> for #ty {
            fn type_info() -> <::megh::__sqlx::#db as ::megh::__sqlx::Database>::TypeInfo {
                <#stored as ::megh::__sqlx::Type<::megh::__sqlx::#db>>::type_info()
            }
            fn compatible(ty: &<::megh::__sqlx::#db as ::megh::__sqlx::Database>::TypeInfo) -> bool {
                <#stored as ::megh::__sqlx::Type<::megh::__sqlx::#db>>::compatible(ty)
            }
        }
    }
}

fn encode_impl(ty: &Ident, db: &Ident, Mapping { stored, write, .. }: &Mapping) -> TokenStream {
    let (input, body) = (&write.inputs, block(&write.body));
    quote! {
        impl<'q> ::megh::__sqlx::Encode<'q, ::megh::__sqlx::#db> for #ty {
            fn encode_by_ref(
                &self,
                buf: &mut <::megh::__sqlx::#db as ::megh::__sqlx::Database>::ArgumentBuffer<'q>,
            ) -> ::std::result::Result<::megh::__sqlx::encode::IsNull, ::megh::__sqlx::error::BoxDynError> {
                fn write(#input) -> #stored #body
                <#stored as ::megh::__sqlx::Encode<'q, ::megh::__sqlx::#db>>::encode(write(self), buf)
            }
        }
    }
}

fn decode_impl(ty: &Ident, db: &Ident, Mapping { stored, read, .. }: &Mapping) -> TokenStream {
    let (input, body) = (&read.inputs, block(&read.body));
    quote! {
        impl<'r> ::megh::__sqlx::Decode<'r, ::megh::__sqlx::#db> for #ty {
            fn decode(
                value: <::megh::__sqlx::#db as ::megh::__sqlx::Database>::ValueRef<'r>,
            ) -> ::std::result::Result<Self, ::megh::__sqlx::error::BoxDynError> {
                fn read(#input) -> ::std::result::Result<#ty, ::megh::__sqlx::error::BoxDynError> #body
                read(<#stored as ::megh::__sqlx::Decode<'r, ::megh::__sqlx::#db>>::decode(value)?)
            }
        }
    }
}
