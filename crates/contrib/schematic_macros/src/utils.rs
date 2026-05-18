use convert_case::{Boundary, Case, Casing};
use quote::{ToTokens, format_ident, quote};
use syn::{
    AngleBracketedGenericArguments, Attribute, Expr, ExprLit, Lit, Meta, Path, PathArguments, Type,
    TypePath,
};

pub fn format_case(format: &str, value: &str, is_variant: bool) -> String {
    let case = match format {
        "lowercase" => {
            return value.to_lowercase();
        }
        "UPPERCASE" => {
            return value.to_uppercase();
        }
        "PascalCase" => Case::Pascal,
        "camelCase" => Case::Camel,
        "snake_case" => Case::Snake,
        "SCREAMING_SNAKE_CASE" => Case::UpperSnake,
        "SCREAMING-KEBAB-CASE" => Case::UpperKebab,
        _ => Case::Kebab,
    };

    value
        .from_case(if is_variant {
            Case::Pascal
        } else {
            Case::Snake
        })
        .remove_boundaries(&[Boundary::UpperDigit, Boundary::LowerDigit])
        .to_case(case)
}

pub fn preserve_str_literal(meta: &Meta) -> darling::Result<Expr> {
    match meta {
        Meta::Path(_) => Err(darling::Error::unsupported_format("path").with_span(meta)),
        Meta::List(_) => Err(darling::Error::unsupported_format("list").with_span(meta)),
        Meta::NameValue(nv) => Ok(nv.value.clone()),
    }
}

pub fn get_meta_path(meta: &Meta) -> &Path {
    match meta {
        Meta::Path(path) => path,
        Meta::List(list) => &list.path,
        Meta::NameValue(nv) => &nv.path,
    }
}

pub fn extract_common_attrs(attrs: &[Attribute]) -> Vec<&Attribute> {
    let preserve = ["allow", "default", "deprecated", "doc", "warn"];

    attrs
        .iter()
        .filter(|a| {
            let path = get_meta_path(&a.meta);

            preserve.iter().any(|n| path.is_ident(n))
        })
        .collect()
}

pub fn extract_comment(attrs: &[&Attribute]) -> Option<String> {
    let mut lines = vec![];

    for attr in attrs {
        if let Meta::NameValue(meta) = &attr.meta
            && meta.path.is_ident("doc")
            && let Expr::Lit(ExprLit {
                lit: Lit::Str(value),
                ..
            }) = &meta.value
        {
            for mut line in value.value().split('\n') {
                line = line.trim();

                if line.starts_with("* ") {
                    line = &line[2..];
                } else if line.starts_with(" * ") {
                    line = &line[3..];
                }

                lines.push(line.to_owned());
            }
        }
    }

    if lines.is_empty() {
        None
    } else {
        Some(lines.join("\n"))
    }
}

pub fn extract_deprecated(attrs: &[&Attribute]) -> Option<String> {
    for attr in attrs {
        match &attr.meta {
            Meta::NameValue(meta) => {
                if meta.path.is_ident("deprecated")
                    && let Expr::Lit(lit) = &meta.value
                {
                    match &lit.lit {
                        Lit::Bool(value) if value.value() => {
                            return Some(String::new()); // No message, handle in renderer
                        }
                        Lit::Str(value) => {
                            return Some(value.value().trim().to_owned());
                        }
                        _ => {}
                    }
                }
            }
            Meta::Path(_) if get_meta_path(&attr.meta).is_ident("deprecated") => {
                return Some(String::new()); // No message, handle in renderer
            }
            _ => {}
        }
    }

    None
}

#[cfg(feature = "schema")]
pub fn map_bool_field_quote(name: &str, value: bool) -> Option<proc_macro2::TokenStream> {
    if value {
        let id = format_ident!("{}", name);

        Some(quote! {
            field.#id = true;

        })
    } else {
        None
    }
}

#[cfg(feature = "schema")]
pub fn map_vec_field_quote(name: &str, value: &[String]) -> Option<proc_macro2::TokenStream> {
    if value.is_empty() {
        None
    } else {
        let id = format_ident!("{}", name);

        Some(quote! {
            field.#id = [
                #(#value),*
            ].into_iter().map(|v| v.to_string()).collect::<Vec<_>>();

        })
    }
}

pub fn map_option_field_quote<T: ToTokens>(
    name: &str,
    value: Option<T>,
) -> Option<proc_macro2::TokenStream> {
    match value {
        Some(value) => {
            let id = format_ident!("{}", name);

            Some(quote! {
                field.#id = Some(#value.into());

            })
        }
        _ => None,
    }
}

// pub fn map_option_variant_quote<T: ToTokens>(
//     name: &str,
//     value: Option<T>,
// ) -> Option<proc_macro2::TokenStream> {
//     if let Some(value) = value {
//         let id = format_ident!("{}", name);

//         Some(quote! {
//             #id: Some(#value.into()),

//         })
//     } else {
//         None
//     }
// }

#[cfg(feature = "schema")]
pub fn map_option_argument_quote<T: ToTokens>(value: Option<T>) -> proc_macro2::TokenStream {
    match value {
        Some(value) => {
            quote! { Some(#value.into()) }
        }
        _ => {
            quote! { None }
        }
    }
}

pub fn instrument_quote() -> proc_macro2::TokenStream {
    // The upstream `tracing` macro feature has been dropped in this fork.
    // Kept as a no-op so existing `#instrument` interpolation sites continue
    // to compile without a sweeping rewrite of generated code.
    quote! {}
}

pub fn expr_path_with_turbofish(ty: &Type) -> proc_macro2::TokenStream {
    let Type::Path(TypePath { qself: None, path }) = ty else {
        return quote! { #ty }; // fallback
    };

    let mut base = path.clone();
    let last = base.segments.last_mut().unwrap();

    let args: Option<AngleBracketedGenericArguments> =
        match std::mem::replace(&mut last.arguments, PathArguments::None) {
            PathArguments::AngleBracketed(a) => Some(a),
            _ => None,
        };

    // e.g., `VecWithMerge :: <T>` when generic args exist, `#base` otherwise.
    if let Some(a) = args {
        quote! { #base :: #a }
    } else {
        quote! { #base }
    }
}
