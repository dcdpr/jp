mod variant;

use darling::FromDeriveInput;
use proc_macro::TokenStream;
use quote::quote;
use syn::{Data, DeriveInput, parse_macro_input};

use crate::{common::ContainerSerdeArgs, config_enum::variant::Variant};

// #[config()]
#[derive(FromDeriveInput, Default)]
#[darling(default, attributes(config), supports(enum_unit, enum_tuple))]
pub struct ConfigEnumArgs {
    before_parse: Option<String>,

    // serde
    rename: Option<String>,
    rename_all: Option<String>,
    rename_all_fields: Option<String>,
}

// #[derive(ConfigEnum)]
pub fn macro_impl(item: TokenStream) -> TokenStream {
    let input: DeriveInput = parse_macro_input!(item);
    let args = ConfigEnumArgs::from_derive_input(&input).expect("Failed to parse arguments.");
    let serde_args = ContainerSerdeArgs::from_derive_input(&input).unwrap_or_default();

    let Data::Enum(data) = input.data else {
        panic!("Only unit enums are supported.");
    };

    #[cfg(feature = "schema")]
    let attrs = crate::utils::extract_common_attrs(&input.attrs);

    let enum_name = &input.ident;

    #[cfg(feature = "schema")]
    let meta_name = args
        .rename
        .as_deref()
        .or(serde_args.rename.as_deref())
        .map(std::borrow::ToOwned::to_owned)
        .unwrap_or(enum_name.to_string());

    let casing_format = args
        .rename_all_fields
        .as_deref()
        .or(args.rename_all.as_deref())
        .or(serde_args.rename_all_fields.as_deref())
        .or(serde_args.rename_all.as_deref())
        .unwrap_or("kebab-case");

    let variants = data
        .variants
        .iter()
        .map(|v| Variant::from(v, casing_format))
        .collect::<Vec<_>>();

    let collected = collect_variant_tokens(variants);
    let before_parse = before_parse_quote(args.before_parse.as_deref());
    let from_fallback = from_fallback_quote(collected.has_fallback);

    let mut impls = vec![emit_enum_impls(
        enum_name,
        &collected,
        &before_parse,
        &from_fallback,
    )];

    #[cfg(feature = "schema")]
    impls.push(emit_enum_schematic_impl(
        enum_name,
        &meta_name,
        &attrs,
        &collected.schema_types,
        collected.default_index,
    ));

    #[cfg(not(feature = "schema"))]
    impls.push(quote! {
        #[automatically_derived]
        impl schematic::Schematic for #enum_name {}
    });

    quote! {
        #(#impls)*
    }
    .into()
}

struct CollectedVariants {
    unit_names: Vec<proc_macro2::TokenStream>,
    display_stmts: Vec<proc_macro2::TokenStream>,
    from_stmts: Vec<proc_macro2::TokenStream>,
    schema_types: Vec<proc_macro2::TokenStream>,
    has_fallback: bool,
    #[cfg(feature = "schema")]
    default_index: Option<usize>,
}

fn collect_variant_tokens(variants: Vec<Variant<'_>>) -> CollectedVariants {
    let mut unit_names = vec![];
    let mut display_stmts = vec![];
    let mut from_stmts = vec![];
    let mut schema_types = vec![];
    let mut has_fallback = false;
    #[cfg(feature = "schema")]
    let mut default_index = None;

    #[cfg_attr(not(feature = "schema"), allow(unused_variables))]
    for (index, variant) in variants.into_iter().enumerate() {
        unit_names.push(variant.get_unit_name());
        display_stmts.push(variant.get_display_fmt());
        from_stmts.push(variant.get_from_str());
        schema_types.push(variant.get_schema_type());

        #[cfg(feature = "schema")]
        if variant.default {
            default_index = Some(index);
        }

        if variant.args.fallback {
            assert!(!has_fallback, "Only 1 fallback variant is supported.");

            has_fallback = true;
        }
    }

    CollectedVariants {
        unit_names,
        display_stmts,
        from_stmts,
        schema_types,
        has_fallback,
        #[cfg(feature = "schema")]
        default_index,
    }
}

fn before_parse_quote(parser: Option<&str>) -> proc_macro2::TokenStream {
    match parser {
        Some("lowercase") => quote! {
            let value = value.to_lowercase();
            let value = value.as_str();
        },
        Some("UPPERCASE") => quote! {
            let value = value.to_uppercase();
            let value = value.as_str();
        },
        Some(other) => panic!("Unknown `before_parse` value {other}"),
        None => quote! {},
    }
}

fn from_fallback_quote(has_fallback: bool) -> proc_macro2::TokenStream {
    if has_fallback {
        quote! {}
    } else {
        quote! {
            unknown => return Err(schematic::ConfigError::EnumUnknownVariant(unknown.to_owned())),
        }
    }
}

fn emit_enum_impls(
    enum_name: &syn::Ident,
    collected: &CollectedVariants,
    before_parse: &proc_macro2::TokenStream,
    from_fallback: &proc_macro2::TokenStream,
) -> proc_macro2::TokenStream {
    let unit_names = &collected.unit_names;
    let display_stmts = &collected.display_stmts;
    let from_stmts = &collected.from_stmts;

    quote! {
        #[automatically_derived]
        impl schematic::ConfigEnum for #enum_name {
            fn variants() -> Vec<#enum_name> {
                vec![
                    #(#unit_names),*
                ]
            }
        }

        #[automatically_derived]
        impl std::str::FromStr for #enum_name {
            type Err = schematic::ConfigError;

            fn from_str(value: &str) -> std::result::Result<Self, Self::Err> {
                #before_parse
                Ok(match value {
                    #(#from_stmts)*
                    #from_fallback
                })
            }
        }

        #[automatically_derived]
        impl std::convert::TryFrom<String> for #enum_name {
            type Error = schematic::ConfigError;

            fn try_from(value: String) -> std::result::Result<Self, Self::Error> {
                std::str::FromStr::from_str(&value)
            }
        }

        #[automatically_derived]
        impl std::convert::TryFrom<&String> for #enum_name {
            type Error = schematic::ConfigError;

            fn try_from(value: &String) -> std::result::Result<Self, Self::Error> {
                std::str::FromStr::from_str(value)
            }
        }

        #[automatically_derived]
        impl std::convert::TryFrom<&str> for #enum_name {
            type Error = schematic::ConfigError;

            fn try_from(value: &str) -> std::result::Result<Self, Self::Error> {
                std::str::FromStr::from_str(value)
            }
        }

        #[automatically_derived]
        impl std::fmt::Display for #enum_name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "{}", match self {
                    #(#display_stmts)*
                })
            }
        }
    }
}

#[cfg(feature = "schema")]
fn emit_enum_schematic_impl(
    enum_name: &syn::Ident,
    meta_name: &str,
    attrs: &[&syn::Attribute],
    schema_types: &[proc_macro2::TokenStream],
    default_index: Option<usize>,
) -> proc_macro2::TokenStream {
    use crate::utils::{
        extract_comment, extract_deprecated, instrument_quote, map_option_argument_quote,
    };

    let default_index = map_option_argument_quote(default_index);
    let instrument = instrument_quote();

    let deprecated = if let Some(comment) = extract_deprecated(attrs) {
        quote! { schema.set_deprecated(#comment); }
    } else {
        quote! {}
    };
    let description = if let Some(comment) = extract_comment(attrs) {
        quote! { schema.set_description(#comment); }
    } else {
        quote! {}
    };

    quote! {
        #[automatically_derived]
        impl schematic::Schematic for #enum_name {
            fn schema_name() -> Option<String> {
                Some(#meta_name.into())
            }

            #instrument
            fn build_schema(mut schema: schematic::SchemaBuilder) -> schematic::Schema {
                use schematic::schema::*;
                #deprecated
                #description
                schema.enumerable(EnumType::from_fields(
                    [
                        #(#schema_types),*
                    ],
                    #default_index,
                ))
            }
        }
    }
}
