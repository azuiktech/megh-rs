//! `sqlx_type!`: a type stored per database through a write and a read closure, expanded to sqlx's `Type`, `Encode`
//! and `Decode`.

use proc_macro2::{Span, TokenStream};
use quote::{format_ident, quote};
use syn::parse::{Parse, ParseStream};
use crate::parse::crate_path;
use syn::{Arm, Expr, ExprClosure, ExprMatch, Ident, Pat, ReturnType, Token, Type};

type Arms = Vec<(Option<String>, Mapping)>;

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
    scrutinee(&spec.arms.expr)?;
    let arms = spec.arms.arms.iter().try_fold(Arms::new(), add_arm)?;
    Ok(DRIVERS.iter().filter_map(|name| arm_for(&arms, name).map(|mapping| impls(&spec.ty, name, mapping))).collect())
}

/// The `match` is on `driver`.
fn scrutinee(expr: &Expr) -> syn::Result<()> {
    match expr {
        Expr::Path(path) if path.path.is_ident("driver") => Ok(()),
        other => Err(syn::Error::new_spanned(other, "expected `match driver`")),
    }
}

/// `arms` with `arm` added; a guard or a driver (or `_`) named twice is an error.
fn add_arm(mut arms: Arms, arm: &Arm) -> syn::Result<Arms> {
    let driver = driver(&arm.pat)?;
    match arms.iter().any(|(named, _)| *named == driver) {
        true => Err(syn::Error::new_spanned(&arm.pat, "this driver already has an arm")),
        false => {
            arms.push((driver, mapping(arm)?));
            Ok(arms)
        }
    }
}

/// The arm for `name`, else the `_` arm.
fn arm_for<'a>(arms: &'a Arms, name: &str) -> Option<&'a Mapping> {
    arms.iter()
        .find(|(driver, _)| driver.as_deref() == Some(name))
        .or_else(|| arms.iter().find(|(driver, _)| driver.is_none()))
        .map(|(_, mapping)| mapping)
}

/// The driver an arm names, `None` for `_`.
fn driver(pat: &Pat) -> syn::Result<Option<String>> {
    match pat {
        Pat::Wild(_) => Ok(None),
        Pat::Guard(pat) => Err(syn::Error::new_spanned(&pat.guard, "an arm takes no guard")),
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
        ReturnType::Type(_, stored) if matches!(**stored, Type::Reference(_)) => {
            Err(syn::Error::new_spanned(stored, "the stored type is owned, not a reference"))
        }
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

/// A closure whose one parameter has a type; it becomes a function, so it captures nothing.
fn closure(expr: &Expr) -> syn::Result<ExprClosure> {
    match expr {
        Expr::Closure(closure) if closure.inputs.len() == 1 && matches!(closure.inputs[0], Pat::Type(_)) => Ok(closure.clone()),
        Expr::Closure(closure) => Err(syn::Error::new_spanned(&closure.inputs, "expected one typed parameter: `|value: &T|`")),
        other => Err(syn::Error::new_spanned(other, "expected a closure")),
    }
}

/// `Type`, `Encode` and `Decode` of `ty` for one driver (and `PgHasArrayType` for `Postgres`), kept only when megh has
/// that driver's feature.
fn impls(ty: &Ident, driver: &str, mapping: &Mapping) -> TokenStream {
    let krate = crate_path();
    let gate = format_ident!("__if_{}", driver.to_lowercase());
    let db = Ident::new(driver, Span::call_site());
    let (type_info, encode, decode) = (type_info(ty, &db, mapping), encode(ty, &db, mapping), decode(ty, &db, mapping));
    let array = (driver == "Postgres").then(|| array_type(ty, mapping));
    quote! { #krate::#gate! { #type_info #encode #decode #array } }
}

fn array_type(ty: &Ident, Mapping { stored, .. }: &Mapping) -> TokenStream {
    let krate = crate_path();
    quote! {
        impl #krate::__sqlx::postgres::PgHasArrayType for #ty {
            fn array_type_info() -> #krate::__sqlx::postgres::PgTypeInfo {
                <#stored as #krate::__sqlx::postgres::PgHasArrayType>::array_type_info()
            }
            fn array_compatible(ty: &#krate::__sqlx::postgres::PgTypeInfo) -> bool {
                <#stored as #krate::__sqlx::postgres::PgHasArrayType>::array_compatible(ty)
            }
        }
    }
}

fn type_info(ty: &Ident, db: &Ident, Mapping { stored, .. }: &Mapping) -> TokenStream {
    let krate = crate_path();
    quote! {
        impl #krate::__sqlx::Type<#krate::__sqlx::#db> for #ty {
            fn type_info() -> <#krate::__sqlx::#db as #krate::__sqlx::Database>::TypeInfo {
                <#stored as #krate::__sqlx::Type<#krate::__sqlx::#db>>::type_info()
            }
            fn compatible(ty: &<#krate::__sqlx::#db as #krate::__sqlx::Database>::TypeInfo) -> bool {
                <#stored as #krate::__sqlx::Type<#krate::__sqlx::#db>>::compatible(ty)
            }
        }
    }
}

fn encode(ty: &Ident, db: &Ident, Mapping { stored, write, .. }: &Mapping) -> TokenStream {
    let krate = crate_path();
    let (input, body) = (&write.inputs, block(&write.body));
    quote! {
        impl<'q> #krate::__sqlx::Encode<'q, #krate::__sqlx::#db> for #ty {
            fn encode_by_ref(
                &self,
                buf: &mut <#krate::__sqlx::#db as #krate::__sqlx::Database>::ArgumentBuffer<'q>,
            ) -> ::std::result::Result<#krate::__sqlx::encode::IsNull, #krate::__sqlx::error::BoxDynError> {
                fn write(#input) -> #stored #body
                <#stored as #krate::__sqlx::Encode<'q, #krate::__sqlx::#db>>::encode(write(self), buf)
            }
        }
    }
}

fn decode(ty: &Ident, db: &Ident, Mapping { stored, read, .. }: &Mapping) -> TokenStream {
    let krate = crate_path();
    let (input, body) = (&read.inputs, block(&read.body));
    quote! {
        impl<'r> #krate::__sqlx::Decode<'r, #krate::__sqlx::#db> for #ty {
            fn decode(
                value: <#krate::__sqlx::#db as #krate::__sqlx::Database>::ValueRef<'r>,
            ) -> ::std::result::Result<Self, #krate::__sqlx::error::BoxDynError> {
                fn read(#input) -> ::std::result::Result<#ty, #krate::__sqlx::error::BoxDynError> #body
                read(<#stored as #krate::__sqlx::Decode<'r, #krate::__sqlx::#db>>::decode(value)?)
            }
        }
    }
}
