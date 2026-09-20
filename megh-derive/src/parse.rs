//! Parsing attributes and struct definitions for Table derive macro.

use heck::{ToKebabCase, ToLowerCamelCase, ToShoutySnakeCase, ToSnakeCase, ToUpperCamelCase};
use syn::{Attribute, DeriveInput, Field, Fields, LitStr};

#[derive(Debug, Clone)]
pub struct ParsedColumn {
    pub name: String,
    pub is_primary_key: bool,
    pub is_json: bool,
    pub has_default: bool,
}

#[derive(Debug, Clone)]
pub struct TableConfig {
    pub table_name: String,
    pub primary_keys: Vec<String>,
    pub columns: Vec<ParsedColumn>,
}

impl TableConfig {
    pub fn from_input(input: &DeriveInput) -> Result<Self, syn::Error> {
        let (name_attr, mut explicit_keys, rename_all) = parse_container_attrs(&input.attrs)?;
        let table_name = name_attr.unwrap_or_else(|| input.ident.to_string().to_snake_case());

        let fields = match &input.data {
            syn::Data::Struct(s) => match &s.fields {
                Fields::Named(named) => &named.named,
                _ => return Err(syn::Error::new_spanned(input, "Table only supports named fields")),
            },
            _ => return Err(syn::Error::new_spanned(input, "Table only supports structs")),
        };

        let mut columns = Vec::new();
        for field in fields {
            if let Some(col) = parse_field(field, rename_all.as_deref())? {
                if col.is_primary_key && !explicit_keys.contains(&col.name) {
                    explicit_keys.push(col.name.clone());
                }
                columns.push(col);
            }
        }

        if explicit_keys.is_empty() {
            explicit_keys.push("id".to_string());
        }

        for col in &mut columns {
            if explicit_keys.contains(&col.name) {
                col.is_primary_key = true;
            }
        }

        Ok(TableConfig { table_name, primary_keys: explicit_keys, columns })
    }
}

fn parse_container_attrs(attrs: &[Attribute]) -> Result<(Option<String>, Vec<String>, Option<String>), syn::Error> {
    let mut table_name = None;
    let mut keys = Vec::new();
    let mut rename_all = None;

    for attr in attrs {
        if attr.path().is_ident("table") {
            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("name") {
                    let val: LitStr = meta.value()?.parse()?;
                    table_name = Some(val.value());
                    Ok(())
                } else if meta.path.is_ident("keys") {
                    let val = meta.value()?;
                    let content;
                    syn::bracketed!(content in val);
                    let items = syn::punctuated::Punctuated::<LitStr, syn::Token![,]>::parse_terminated(&content)?;
                    keys = items.into_iter().map(|s| s.value()).collect();
                    Ok(())
                } else {
                    Ok(())
                }
            })?;
        } else if attr.path().is_ident("sqlx") {
            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("rename_all") {
                    let val: LitStr = meta.value()?.parse()?;
                    rename_all = Some(val.value());
                }
                Ok(())
            })?;
        }
    }
    Ok((table_name, keys, rename_all))
}

fn parse_field(field: &Field, rename_all: Option<&str>) -> Result<Option<ParsedColumn>, syn::Error> {
    let ident = field.ident.as_ref().unwrap().to_string();
    let mut name = None;
    let mut skip = false;
    let mut is_json = false;
    let mut has_default = false;
    let mut is_primary_key = false;

    for attr in &field.attrs {
        if attr.path().is_ident("sqlx") {
            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("skip") {
                    skip = true;
                } else if meta.path.is_ident("rename") {
                    let val: LitStr = meta.value()?.parse()?;
                    name = Some(val.value());
                } else if meta.path.is_ident("json") {
                    is_json = true;
                } else if meta.path.is_ident("default") {
                    has_default = true;
                } else if meta.input.peek(syn::Token![=]) {
                    let _ = meta.value()?.parse::<syn::Expr>();
                }
                Ok(())
            })?;
        } else if attr.path().is_ident("key") || attr.path().is_ident("table") {
            is_primary_key = true;
        }
    }

    if skip {
        return Ok(None);
    }

    let col_name = name.unwrap_or_else(|| apply_rename_all(&ident, rename_all));
    Ok(Some(ParsedColumn { name: col_name, is_primary_key, is_json, has_default }))
}

fn apply_rename_all(name: &str, rename_all: Option<&str>) -> String {
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
