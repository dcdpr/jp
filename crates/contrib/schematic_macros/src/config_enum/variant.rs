use darling::FromAttributes;
use proc_macro2::{Ident, TokenStream};
use quote::quote;
use syn::{Attribute, Fields, Variant as NativeVariant};

use crate::{
    common::FieldSerdeArgs,
    utils::{
        extract_comment, extract_common_attrs, extract_deprecated, format_case, get_meta_path,
        map_option_field_quote,
    },
};

// #[variant()]
#[derive(FromAttributes, Default)]
#[darling(default, attributes(variant))]
pub struct VariantArgs {
    pub fallback: bool,
    pub value: Option<String>,

    /// Extra spellings `FromStr` accepts for this variant.
    ///
    /// Written as `#[variant(aliases("strip_responses", "sres"))]`.
    /// The schema keeps the canonical value as the variant's literal and lists
    /// these beside it, so a consumer can offer one and still accept all.
    pub aliases: Vec<syn::LitStr>,
}

pub struct Variant<'l> {
    pub args: VariantArgs,
    #[cfg_attr(not(feature = "schema"), allow(dead_code))]
    pub default: bool,
    /// Whether serde leaves this variant out of the wire vocabulary.
    ///
    /// `FromStr` and `Display` still handle it, so a variant reserved for
    /// internal use stays constructible from a string in Rust while never being
    /// offered to, or accepted from, a user.
    pub skipped: bool,
    pub serde_args: FieldSerdeArgs,
    pub attrs: Vec<&'l Attribute>,
    pub name: &'l Ident,
    pub value: String,
}

impl Variant<'_> {
    pub fn from<'n>(variant: &'n NativeVariant, format: &str) -> Variant<'n> {
        let args = VariantArgs::from_attributes(&variant.attrs).unwrap_or_default();
        let serde_args = FieldSerdeArgs::from_attributes(&variant.attrs).unwrap_or_default();

        if args.fallback {
            match &variant.fields {
                Fields::Unnamed(fields) => {
                    assert!(
                        fields.unnamed.len() == 1,
                        "Only 1 unnamed field is supported for `fallback`."
                    );
                }
                _ => {
                    panic!("Only unnamed tuple variants are supported for `fallback`.");
                }
            }

            assert!(
                args.value.is_none(),
                "`value` is not supported for `fallback`."
            );
        } else if !matches!(variant.fields, Fields::Unit) {
            panic!("Only unit variants are supported.");
        }

        let value = if args.fallback {
            String::new()
        } else if let Some(v) = &args.value {
            v.to_owned()
        } else if let Some(v) = &serde_args.rename {
            v.to_owned()
        } else {
            format_case(format, variant.ident.to_string().as_str(), true)
        };

        let attrs = extract_common_attrs(&variant.attrs);

        Variant {
            default: attrs
                .iter()
                .any(|v| get_meta_path(&v.meta).is_ident("default")),
            skipped: serde_args.skip,
            attrs,
            name: &variant.ident,
            value,
            args,
            serde_args,
        }
    }

    /// Every spelling other than the canonical one that `FromStr` accepts.
    pub fn aliases(&self) -> Vec<String> {
        let mut aliases: Vec<String> = self.args.aliases.iter().map(syn::LitStr::value).collect();

        if let Some(alias) = &self.serde_args.alias
            && !aliases.contains(alias)
        {
            aliases.push(alias.clone());
        }

        aliases
    }

    pub fn get_display_fmt(&self) -> TokenStream {
        let name = &self.name;
        let value = &self.value;

        if self.args.fallback {
            quote! {
                Self::#name(fallback) => return std::write!(f, "{fallback}"),
            }
        } else {
            quote! {
                Self::#name => #value,
            }
        }
    }

    pub fn get_from_str(&self) -> TokenStream {
        let name = &self.name;
        let value = &self.value;

        if self.args.fallback {
            return quote! {
                fallback => Self::#name(
                    fallback.try_into().map_err(|_| {
                        schematic::ConfigError::EnumInvalidFallback(fallback.to_string())
                    })?
                ),
            };
        }

        let spellings = self.aliases();

        quote! {
            #value #(| #spellings)* => Self::#name,
        }
    }

    pub fn get_schema_type(&self) -> TokenStream {
        let name = self.name.to_string();
        let comment = map_option_field_quote("comment", extract_comment(&self.attrs));
        let deprecated = map_option_field_quote("deprecated", extract_deprecated(&self.attrs));
        let aliases = crate::utils::map_vec_field_quote("aliases", &self.aliases());

        let inner_schema = if self.args.fallback {
            quote! {
                Schema::string(StringType::default())
            }
        } else {
            let value = &self.value;

            quote! {
                Schema::literal_value(LiteralValue::String(#value.into()))
            }
        };

        if comment.is_none() && deprecated.is_none() && aliases.is_none() {
            quote! {
                (#name.into(), SchemaField::new(#inner_schema))
            }
        } else {
            quote! {
                (#name.into(), {
                    let mut field = SchemaField::new(#inner_schema);
                    #comment
                    #deprecated
                    #aliases
                    field
                })
            }
        }
    }

    pub fn get_unit_name(&self) -> TokenStream {
        let name = &self.name;

        if self.args.fallback {
            quote! {
                Self::#name(Default::default())
            }
        } else {
            quote! {
                Self::#name
            }
        }
    }
}
