use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::{Expr, Lit};

use crate::common::{FieldArgs, FieldValue, TypeInfo};

impl FieldValue {
    pub fn generate_empty_value(&self) -> TokenStream {
        match self {
            Self::NestedMap {
                collection,
                collection_info,
                ..
            }
            | Self::NestedList {
                collection,
                collection_info,
                ..
            } => {
                if collection_info.optional {
                    quote! { None }
                } else {
                    quote! { #collection::default() }
                }
            }
            Self::NestedValue { info, .. } => {
                if info.optional {
                    quote! { None }
                } else {
                    let partial_name = format_ident!("Partial{}", info.config.as_ref().unwrap());
                    quote! { #partial_name::empty() }
                }
            }
            Self::Value { .. } => quote! { None },
        }
    }

    pub fn generate_default_value(
        &self,
        args: &FieldArgs,
        nullable: bool,
        required: bool,
    ) -> TokenStream {
        let (value, nested) = match self {
            Self::NestedList { collection, .. } | Self::NestedMap { collection, .. } => {
                (quote!(#collection), false)
            }
            Self::NestedValue { info, .. } => {
                let partial_name = format_ident!("Partial{}", info.config.as_ref().unwrap());
                (quote!(#partial_name), true)
            }
            Self::Value { value, .. } => (quote!(#value), false),
        };

        match args.default.as_ref() {
            Some(expr) => match expr {
                Expr::Array(_) | Expr::Call(_) | Expr::Macro(_) | Expr::Tuple(_) => {
                    quote! { Some(#expr) }
                }
                Expr::Path(func) => {
                    quote! { handle_default_result(#func(context))? }
                }
                Expr::Lit(lit) => match &lit.lit {
                    Lit::Str(string) => {
                        if nested && !nullable {
                            quote! {
                                handle_default_result(#value::try_from(#string))?
                            }
                        } else {
                            quote! {
                                Some(handle_default_result(#value::try_from(#string))?)
                            }
                        }
                    }
                    other => quote! { Some(#other) },
                },
                invalid => {
                    let info = format!("{invalid:?}");

                    panic!(
                        "Unsupported default value ({info}). May only provide literals, \
                         primitives, arrays, or tuples."
                    );
                }
            },
            _ => {
                if nullable {
                    quote! { None }
                } else if nested {
                    let default = if required {
                        quote! { #value::default() }
                    } else {
                        quote! { #value::default_values(context)?.unwrap_or_default() }
                    };
                    if matches!(self, Self::NestedValue { info, .. } if info.boxed) {
                        quote! { Box::new(#default) }
                    } else {
                        default
                    }
                } else if required {
                    quote! { None }
                } else {
                    quote! { Default::default() }
                }
            }
        }
    }

    pub fn generate_env_value(&self, args: &FieldArgs, env_key: &str) -> Option<TokenStream> {
        match self {
            Self::NestedValue { info, .. } => {
                let partial_name = format_ident!("Partial{}", info.config.as_ref().unwrap());

                Some(if info.optional {
                    let value = quote! { track_env(#partial_name::env_values()?, &mut tracker) };
                    if info.boxed {
                        quote! { #value.map(Box::new) }
                    } else {
                        value
                    }
                } else {
                    quote! { track_env(#partial_name::env_values()?, &mut tracker).unwrap_or_default() }
                })
            }
            Self::Value { .. } => Some(match &args.parse_env {
                Some(parse_env) => {
                    quote! {
                        track_env(parse_env_value(#env_key, #parse_env)?, &mut tracker)
                    }
                }
                _ => {
                    quote! {
                        track_env(default_env_value(#env_key)?, &mut tracker)
                    }
                }
            }),
            _ => None,
        }
    }

    pub fn get_from_partial_value(&self, field: &str) -> TokenStream {
        let push_field = if field.is_empty() {
            quote! {}
        } else {
            quote! { fields.push(#field.to_owned()); }
        };

        match self {
            Self::NestedList {
                item, item_info, ..
            } => self.map_data_with_info(
                quote! {
                    #item::from_partial(value, {
                        let mut fields = fields.clone();
                        #push_field
                        fields.push(index.to_string());
                        fields
                    })?
                },
                item_info,
            ),
            Self::NestedMap {
                value, value_info, ..
            } => self.map_data_with_info(
                quote! {
                    #value::from_partial(value, {
                        let mut fields = fields.clone();
                        #push_field
                        fields.push(key.to_string());
                        fields
                    })?
                },
                value_info,
            ),
            Self::NestedValue { info, .. } => {
                let config = info.config.as_ref();
                let data = if info.boxed {
                    quote! { *data }
                } else {
                    quote! { data }
                };
                quote! {
                    #config::from_partial(#data, {
                        let mut fields = fields.clone();
                        #push_field
                        fields
                    })?
                }
            }
            Self::Value { .. } => quote! { data },
        }
    }

    pub fn get_merge_statement(&self, key: &TokenStream, args: &FieldArgs) -> TokenStream {
        match self {
            Self::NestedValue { info, .. } => merge_nested_value_stmt(key, info, args),
            Self::NestedList {
                collection_info, ..
            }
            | Self::NestedMap {
                collection_info, ..
            } => merge_collection_stmt(key, collection_info, args),
            Self::Value { .. } => merge_value_stmt(key, args),
        }
    }

    pub fn map_data(&self, mapped_data: &TokenStream) -> TokenStream {
        match self {
            Self::NestedList { collection, .. } => {
                let method = if collection.to_string().ends_with("Set") {
                    quote! { insert }
                } else {
                    quote! { push }
                };
                quote! {
                    {
                        let mut result = #collection::default();
                        for (index, value) in data.into_iter().enumerate() {
                            let entry = #mapped_data;
                            result.#method(entry);
                        }
                        result
                    }
                }
            }
            Self::NestedMap { collection, .. } => {
                quote! {
                    {
                        let mut result = #collection::default();
                        for (key, value) in data {
                            let entry = #mapped_data;
                            result.insert(key, entry);
                        }
                        result
                    }
                }
            }
            Self::NestedValue { .. } | Self::Value { .. } => {
                quote! { #mapped_data }
            }
        }
    }

    pub fn map_data_with_info(&self, mapped_data: TokenStream, info: &TypeInfo) -> TokenStream {
        let mut data = mapped_data;

        if info.boxed {
            data = quote! { Box::new(#data) };
        }

        if info.optional {
            data = quote! { Some(#data) };
        }

        self.map_data(&data)
    }
}

fn merge_nested_value_stmt(
    key: &TokenStream,
    info: &super::super::common::field_value::TypeInfo,
    args: &FieldArgs,
) -> TokenStream {
    if let Some(func) = args.merge.as_ref() {
        return if info.optional {
            quote! {
                self.#key = merge_setting(
                    self.#key.take(),
                    next.#key.take(),
                    context,
                    #func,
                )?;
            }
        } else {
            quote! {
                self.#key = merge_nested_map_setting(
                    std::mem::take(&mut self.#key),
                    std::mem::take(&mut next.#key),
                    context,
                    #func,
                )?;
            }
        };
    }

    if info.optional {
        if info.boxed {
            quote! {
                self.#key = merge_nested_optional_setting(
                    self.#key.take().map(|v| *v),
                    next.#key.take().map(|v| *v),
                    context,
                )?.map(Box::new);
            }
        } else {
            quote! {
                self.#key = merge_nested_optional_setting(
                    self.#key.take(),
                    next.#key.take(),
                    context,
                )?;
            }
        }
    } else {
        quote! {
            self.#key = merge_nested_setting(
                std::mem::take(&mut self.#key),
                std::mem::take(&mut next.#key),
                context,
            )?;
        }
    }
}

fn merge_collection_stmt(
    key: &TokenStream,
    collection_info: &super::super::common::field_value::TypeInfo,
    args: &FieldArgs,
) -> TokenStream {
    if let Some(func) = args.merge.as_ref() {
        return if collection_info.optional {
            quote! {
                self.#key = merge_setting(
                    self.#key.take(),
                    next.#key.take(),
                    context,
                    #func,
                )?;
            }
        } else {
            quote! {
                self.#key = merge_nested_map_setting(
                    std::mem::take(&mut self.#key),
                    std::mem::take(&mut next.#key),
                    context,
                    #func,
                )?;
            }
        };
    }

    if collection_info.optional {
        quote! {
            if next.#key.is_some() {
                self.#key = next.#key;
            }
        }
    } else {
        quote! {
            self.#key = next.#key;
        }
    }
}

fn merge_value_stmt(key: &TokenStream, args: &FieldArgs) -> TokenStream {
    if let Some(func) = args.merge.as_ref() {
        quote! {
            self.#key = merge_setting(
                self.#key.take(),
                next.#key.take(),
                context,
                #func,
            )?;
        }
    } else {
        quote! {
            if next.#key.is_some() {
                self.#key = next.#key;
            }
        }
    }
}
