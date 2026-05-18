use proc_macro2::{Ident, TokenStream};
use quote::{ToTokens, quote};
use syn::{GenericArgument, PathArguments, Type};

fn is_collection_type(ident: &Ident) -> bool {
    let name = ident.to_string();

    name.ends_with("Vec") || name.ends_with("Set") || name.ends_with("Map")
}

#[derive(Debug, Default)]
pub struct TypeInfo {
    pub boxed: bool,
    pub optional: bool,
    pub config: Option<Ident>,
}

pub fn extract_inner_type<'a>(ty: &'a Type, info: &mut TypeInfo) -> &'a Type {
    // We don't need to traverse other types, just paths
    let Type::Path(type_path) = ty else {
        return ty;
    };

    // Extract the last segment of the path, for example `Option`,
    // instead of the full path `std::option::Option`
    let last_segment = type_path.path.segments.last().unwrap();

    // If a collection type, return the path immediately, as we'll need
    // to extract inner information later on
    if is_collection_type(&last_segment.ident) {
        return ty;
    }

    // If a wrapper type, mark the information for later
    let mut nested = false;

    if last_segment.ident == "Option" {
        info.optional = true;
        nested = true;
    } else if last_segment.ident == "Box" {
        info.boxed = true;
        nested = true;
    }

    // If a nested type, drill down deeper to find the inner type
    if nested {
        if let PathArguments::AngleBracketed(args) = &last_segment.arguments
            && let GenericArgument::Type(inner_ty) = args.args.last().unwrap()
        {
            return extract_inner_type(inner_ty, info);
        }
    }
    // Otherwise we found the inner type, so extract the ident name
    else {
        info.config = Some(last_segment.ident.clone());
    }

    ty
}

#[derive(Debug)]
pub enum FieldValue {
    // Vec<item>
    NestedList {
        collection: Ident,
        collection_info: TypeInfo,
        item: Type,
        item_info: TypeInfo,
    },
    // HashMap<key, value>
    NestedMap {
        collection: Ident,
        collection_info: TypeInfo,
        key: Type,
        value: Box<Type>,
        value_info: TypeInfo,
    },
    // config
    NestedValue {
        info: TypeInfo,
        value: Type,
    },
    // value
    Value {
        info: TypeInfo,
        value: Type,
    },
}

impl FieldValue {
    pub fn nested(raw: &Type) -> FieldValue {
        let mut outer_info = TypeInfo::default();
        let ty = extract_inner_type(raw, &mut outer_info);

        let Type::Path(ty_path) = ty else {
            panic!("Nested values may only be paths/type references.");
        };

        let segment = ty_path.path.segments.last().unwrap();
        let name = segment.ident.to_string();

        if name.ends_with("Vec") || name.ends_with("Set") {
            let PathArguments::AngleBracketed(args) = &segment.arguments else {
                panic!("Received a {name} without inner arguments!");
            };

            let Some(GenericArgument::Type(inner_ty)) = args.args.first() else {
                panic!("{name} item type must be a path!");
            };

            let mut inner_info = TypeInfo::default();
            let item = extract_inner_type(inner_ty, &mut inner_info);

            Self::NestedList {
                collection: segment.ident.clone(),
                collection_info: outer_info,
                item: item.clone(),
                item_info: inner_info,
            }
        } else if name.ends_with("Map") {
            let PathArguments::AngleBracketed(args) = &segment.arguments else {
                panic!("Received a {name} without inner arguments!");
            };

            let Some(GenericArgument::Type(key_ty)) = args.args.first() else {
                panic!("{name} key type must be a path!");
            };

            let Some(GenericArgument::Type(value_ty)) = args.args.last() else {
                panic!("{name} value type must be a path!");
            };

            let mut inner_info = TypeInfo::default();
            let value = extract_inner_type(value_ty, &mut inner_info);

            Self::NestedMap {
                collection: segment.ident.clone(),
                collection_info: outer_info,
                key: key_ty.clone(),
                value: Box::new(value.clone()),
                value_info: inner_info,
            }
        } else {
            Self::NestedValue {
                info: outer_info,
                value: ty.clone(),
            }
        }
    }

    pub fn value(raw: &Type) -> FieldValue {
        let mut info = TypeInfo::default();
        let value = extract_inner_type(raw, &mut info).clone();

        Self::Value { info, value }
    }

    pub fn is_outer_boxed(&self) -> bool {
        match self {
            Self::NestedList {
                collection_info, ..
            }
            | Self::NestedMap {
                collection_info, ..
            } => collection_info.boxed,
            Self::NestedValue { info, .. } | Self::Value { info, .. } => info.boxed,
        }
    }

    pub fn is_outer_optional(&self) -> bool {
        match self {
            Self::NestedList {
                collection_info, ..
            }
            | Self::NestedMap {
                collection_info, ..
            } => collection_info.optional,
            Self::NestedValue { info, .. } | Self::Value { info, .. } => info.optional,
        }
    }

    pub fn get_config_type(&self) -> &Type {
        match self {
            Self::NestedList { item, .. } => item,
            Self::NestedMap { value, .. } => value.as_ref(),
            Self::NestedValue { value, .. } | Self::Value { value, .. } => value,
        }
    }

    pub fn is_container(&self) -> bool {
        matches!(self, Self::NestedMap { .. } | Self::NestedList { .. })
    }
}

// Only used for partials!!!
impl ToTokens for FieldValue {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        tokens.extend(match self {
            Self::NestedList {
                collection, item, ..
            } => {
                quote! { #collection<<#item as schematic::Config>::Partial> }
            }
            Self::NestedMap {
                collection,
                key,
                value,
                ..
            } => {
                quote! {
                    #collection<#key, <#value as schematic::Config>::Partial>
                }
            }
            Self::NestedValue { value, info } => {
                let partial = if info.boxed {
                    quote! { Box<<#value as schematic::Config>::Partial> }
                } else {
                    quote! { <#value as schematic::Config>::Partial }
                };
                if info.optional {
                    quote! { Option<#partial> }
                } else {
                    partial
                }
            }
            Self::Value { value, .. } => {
                quote! { Option<#value> }
            }
        });
    }
}
