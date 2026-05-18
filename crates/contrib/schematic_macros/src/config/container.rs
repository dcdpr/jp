use proc_macro2::{Ident, Span, TokenStream};
use quote::quote;
use syn::{GenericParam, Generics, Lifetime, parse_quote};

use crate::common::Container;

impl Container<'_> {
    pub fn generate_empty_values(&self) -> TokenStream {
        match self {
            Self::NamedStruct {
                fields: settings, ..
            } => {
                let mut setting_names = vec![];
                let mut setting_values = vec![];
                for setting in settings {
                    setting_names.push(setting.name);
                    setting_values.push(setting.generate_empty_value());
                }

                quote! {
                    Self {
                        #(#setting_names: #setting_values),*
                    }
                }
            }
            Self::UnnamedStruct {
                fields: settings, ..
            } => {
                let none_values = settings.iter().map(|_| quote! { None });

                quote! { Self(#(#none_values),*) }
            }
            Self::Enum { variants } => {
                let empty = variants.iter().filter(|v| v.is_empty()).collect::<Vec<_>>();
                let default = variants.iter().find(|v| v.is_default());
                let nested = variants
                    .iter()
                    .filter(|v| v.is_nested())
                    .collect::<Vec<_>>();
                let variant = if empty.is_empty() {
                    // If there is no `empty` variant, check for `default`.
                    if let Some(default) = default {
                        default
                    // If there is no `default` variant, check for exactly one
                    // `nested`.
                    } else if nested.len() == 1 {
                        nested[0]
                    // Otherwise, either `empty`, `default` or `nested` must be
                    // attributed to a variant.
                    } else {
                        panic!("No variant has been marked as `empty`, `default` or `nested`.")
                    }
                } else if empty.len() > 1 {
                    panic!("Only 1 variant may be marked as `empty`.");
                } else if default.is_some() && !empty.is_empty() {
                    panic!("`empty` not allowed when `default` is present.");
                } else {
                    empty[0]
                };

                let value = variant.generate_empty_value();

                quote! { Self::#value }
            }
        }
    }

    pub fn generate_is_empty_impl(&self) -> TokenStream {
        let mut checks = vec![];

        match self {
            Self::NamedStruct {
                fields: settings, ..
            }
            | Self::UnnamedStruct {
                fields: settings, ..
            } => {
                for setting in settings {
                    checks.push(setting.generate_is_empty());
                }
            }
            Self::Enum { variants } => {
                let is_empty_stmts = variants
                    .iter()
                    .map(super::super::common::variant::Variant::generate_is_empty)
                    .collect::<Vec<_>>();

                checks.push(quote! {
                    match self {
                        #(#is_empty_stmts)*
                    }
                });
            }
        }

        // Triggers for unit structs such as `struct MyStruct {}`.
        if checks.is_empty() {
            checks.push(quote! { true });
        }

        quote! {
            #(#checks) && *
        }
    }

    pub fn generate_default_values(&self) -> TokenStream {
        match self {
            Self::NamedStruct {
                fields: settings, ..
            } => {
                let mut setting_names = vec![];
                let mut default_values = vec![];

                for setting in settings {
                    setting_names.push(setting.name);
                    default_values.push(setting.generate_default_value());
                }

                quote! {
                    Ok(Some(Self {
                        #(#setting_names: #default_values),*
                    }))
                }
            }
            Self::UnnamedStruct {
                fields: settings, ..
            } => {
                let mut default_values = vec![];

                for setting in settings {
                    default_values.push(setting.generate_default_value());
                }

                quote! {
                    Ok(Some(Self(
                        #(#default_values),*
                    )))
                }
            }
            Self::Enum { variants } => {
                let default_variant = variants.iter().find(|v| v.is_default());

                if let Some(variant) = default_variant {
                    let default_value = variant.generate_default_value();

                    quote! {
                        Ok(Some(Self::#default_value))
                    }
                } else {
                    quote! {
                        Ok(None)
                    }
                }
            }
        }
    }

    pub fn generate_env_values(&self) -> TokenStream {
        match self {
            Self::NamedStruct {
                fields: settings, ..
            }
            | Self::UnnamedStruct {
                fields: settings, ..
            } => {
                let env_stmts = settings
                    .iter()
                    .filter_map(super::super::common::field::Field::generate_env_statement)
                    .collect::<Vec<_>>();

                if env_stmts.is_empty() {
                    quote! {
                        Ok(None)
                    }
                } else {
                    quote! {
                        let mut tracker = false;
                        let mut partial = Self::default();
                        #(#env_stmts)*
                        Ok(if !tracker {
                            None
                        } else {
                            Some(partial)
                        })
                    }
                }
            }
            Self::Enum { .. } => {
                quote! {
                    Ok(None)
                }
            }
        }
    }

    pub fn generate_finalize(&self) -> TokenStream {
        match self {
            Self::NamedStruct {
                fields: settings, ..
            }
            | Self::UnnamedStruct {
                fields: settings, ..
            } => {
                let finalize_stmts = settings
                    .iter()
                    .map(super::super::common::field::Field::generate_finalize_statement)
                    .collect::<Vec<_>>();

                let env_statement = if cfg!(feature = "env") {
                    quote! {
                        if let Some(data) = Self::env_values()? {
                            partial.merge(context, data)?;
                        }
                    }
                } else {
                    quote! {}
                };

                quote! {
                    let mut partial = Self::default();

                    if let Some(data) = Self::default_values(context)? {
                        partial.merge(context, data)?;
                    }

                    partial.merge(context, self)?;

                    #env_statement

                    #(#finalize_stmts)*

                    Ok(partial)
                }
            }
            Self::Enum { variants } => {
                if self.has_nested() {
                    let finalize_stmts = variants
                        .iter()
                        .filter_map(
                            super::super::common::variant::Variant::generate_finalize_statement,
                        )
                        .collect::<Vec<_>>();

                    quote! {
                        Ok(match self {
                            #(#finalize_stmts)*
                            _ => self
                        })
                    }
                } else {
                    quote! {
                        Ok(self)
                    }
                }
            }
        }
    }

    pub fn generate_merge(&self) -> TokenStream {
        match self {
            Self::NamedStruct {
                fields: settings, ..
            }
            | Self::UnnamedStruct {
                fields: settings, ..
            } => {
                let merge_stmts = settings
                    .iter()
                    .map(super::super::common::field::Field::generate_merge_statement)
                    .collect::<Vec<_>>();

                quote! {
                    #(#merge_stmts)*
                    Ok(())
                }
            }
            Self::Enum { variants } => {
                let merge_stmts = variants
                    .iter()
                    .filter_map(super::super::common::variant::Variant::generate_merge_statement)
                    .collect::<Vec<_>>();

                if merge_stmts.is_empty() {
                    quote! {
                        *self = next;
                        Ok(())
                    }
                } else {
                    quote! {
                        match self {
                            #(#merge_stmts)*
                            _ => {
                                *self = next;
                            }
                        };
                        Ok(())
                    }
                }
            }
        }
    }

    pub fn generate_from_partial(&self, partial_name: &Ident) -> TokenStream {
        match self {
            Self::NamedStruct {
                fields: settings, ..
            } => {
                let mut setting_names = vec![];
                let mut from_partial_values = vec![];

                for setting in settings {
                    setting_names.push(setting.name);
                    from_partial_values.push(setting.generate_from_partial_value());
                }

                quote! {
                    Ok(Self {
                        #(#setting_names: #from_partial_values),*
                    })
                }
            }
            Self::UnnamedStruct {
                fields: settings, ..
            } => {
                let mut from_partial_values = vec![];

                for setting in settings {
                    from_partial_values.push(setting.generate_from_partial_value());
                }

                quote! {
                    Ok(Self(
                        #(#from_partial_values),*
                    ))
                }
            }
            Self::Enum { variants } => {
                let from_partial_values = variants
                    .iter()
                    .map(|s| s.generate_from_partial_value(partial_name))
                    .collect::<Vec<_>>();

                quote! {
                    Ok(match partial {
                        #(#from_partial_values)*
                    })
                }
            }
        }
    }

    pub fn generate_partial(
        &self,
        partial_name: &Ident,
        partial_attrs: &[TokenStream],
        partial_generics: &Generics,
        deserialize_derive: bool,
        is_untagged: bool,
    ) -> TokenStream {
        let serde = quote! { ::schematic::serde };

        // For any container that has generics, we need to make sure our
        // `Deserialize` attribute is bound to the correct type.
        let serde_bound = if partial_generics.type_params().count() > 0 {
            let bounds_str = partial_generics
                .type_params()
                .map(|tp| format!("{}: ::schematic::serde::de::DeserializeOwned", tp.ident))
                .collect::<Vec<_>>()
                .join(", ");

            Some(quote!(#[serde(bound(deserialize = #bounds_str))]))
        } else {
            None
        };

        let (_, ty_generics, _) = partial_generics.split_for_impl();
        let mut generics = partial_generics.clone();
        let where_clause = generics.make_where_clause();
        for tp in partial_generics.type_params() {
            let ident = &tp.ident;

            where_clause.predicates
                .push(parse_quote!(#ident: Clone + PartialEq + #serde::Serialize + #serde::de::DeserializeOwned));

            if self.has_nested() {
                where_clause
                    .predicates
                    .push(parse_quote!(#ident: schematic::Schematic));
            }
        }

        let de_derive = if deserialize_derive && !is_untagged {
            Some(quote! { #[derive(#serde::Deserialize)] })
        } else {
            None
        };

        match self {
            Self::NamedStruct { fields, .. } => {
                quote! {
                    #[derive(Clone, Debug, PartialEq, #serde::Serialize)]
                    #de_derive
                    #[serde(crate = "::schematic::serde")]
                    #serde_bound
                    #(#partial_attrs)*
                    pub struct #partial_name #ty_generics #where_clause {
                        #(#fields)*
                    }
                }
            }
            Self::UnnamedStruct { fields, .. } => {
                quote! {
                    #[derive(Clone, Debug, Default, PartialEq, #serde::Serialize)]
                    #de_derive
                    #[serde(crate = "::schematic::serde")]
                    #serde_bound
                    #(#partial_attrs)*
                    pub struct #partial_name #ty_generics(
                        #(#fields)*
                    ) #where_clause;
                }
            }
            Self::Enum { variants } => {
                quote! {
                    #[derive(Clone, Debug, PartialEq, #serde::Serialize)]
                    #de_derive
                    #[serde(crate = "::schematic::serde")]
                    #serde_bound
                    #(#partial_attrs)*
                    pub enum #partial_name #ty_generics #where_clause {
                        #(#variants)*
                    }
                }
            }
        }
    }

    pub fn generate_partial_default_impl(
        &self,
        partial_name: &Ident,
        partial_generics: &Generics,
        deserialize_derive: bool,
        is_untagged: bool,
    ) -> TokenStream {
        let serde = quote! { ::schematic::serde };

        let (impl_generics, ty_generics, _) = partial_generics.split_for_impl();
        let mut generics = partial_generics.clone();
        let where_clause = generics.make_where_clause();
        for tp in partial_generics.type_params() {
            let ident = &tp.ident;

            where_clause.predicates
                .push(parse_quote!(#ident: Clone + PartialEq + #serde::Serialize + #serde::de::DeserializeOwned));

            if self.has_nested() {
                where_clause
                    .predicates
                    .push(parse_quote!(#ident: schematic::Schematic));
            }
        }

        match self {
            Self::NamedStruct { fields, .. } | Self::UnnamedStruct { fields, .. } => {
                let defaults = fields.iter().map(|s| {
                    if let Some(name) = &s.name {
                        quote! {
                            #name: Default::default()
                        }
                    } else {
                        quote! {
                            Default::default()
                        }
                    }
                });

                quote! {
                    impl #impl_generics Default for #partial_name #ty_generics #where_clause {
                        fn default() -> Self {
                            Self {
                                #(#defaults),*
                            }
                        }
                    }
                }
            }
            Self::Enum { variants } => {
                let default_variant = variants
                    .iter()
                    .find(|v| v.is_default())
                    .or_else(|| variants.first());

                let default_impl = if let Some(default) = default_variant {
                    let value = default.generate_default_value();
                    quote! { Self::#value }
                } else {
                    quote! { panic!("No variant has been marked as default!"); }
                };

                let deserialize_impl = if deserialize_derive && is_untagged {
                    // For untagged enums, generate custom Deserialize that
                    // collects all errors.
                    self.generate_untagged_deserialize(partial_name, variants, partial_generics)
                } else {
                    quote! {}
                };

                quote! {
                    impl #impl_generics Default for #partial_name #ty_generics #where_clause {
                        fn default() -> Self {
                            #default_impl
                        }
                    }

                    #deserialize_impl
                }
            }
        }
    }

    fn generate_untagged_deserialize(
        &self,
        partial_name: &Ident,
        variants: &[crate::common::Variant<'_>],
        partial_generics: &Generics,
    ) -> TokenStream {
        let variant_attempts: Vec<TokenStream> = variants
            .iter()
            .map(|variant| build_variant_attempt(variant, partial_name))
            .collect();

        let serde = quote! { ::schematic::serde };
        let mut generics1 = partial_generics.clone();
        let mut generics2 = generics1.clone();
        let lt_de = Lifetime::new("'de", Span::call_site());
        generics2
            .params
            .insert(0, GenericParam::Lifetime(syn::LifetimeParam::new(lt_de)));
        let (impl_generics, _, _) = generics2.split_for_impl();

        let where_clause = generics1.make_where_clause();
        for tp in partial_generics.type_params() {
            let ident = &tp.ident;

            where_clause.predicates
                .push(parse_quote!(#ident: Clone + PartialEq + #serde::Serialize + #serde::de::DeserializeOwned));

            if self.has_nested() {
                where_clause
                    .predicates
                    .push(parse_quote!(#ident: schematic::Schematic));
            }
        }
        let where_clause = where_clause.clone();
        let (_, ty_generics, _) = generics1.split_for_impl();

        quote! {
            impl #impl_generics #serde::Deserialize<'de> for #partial_name #ty_generics #where_clause {
            // impl<'de> serde::Deserialize<'de> for #partial_name {
                fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
                where
                    D: serde::Deserializer<'de>,
                {
                    use serde::de::Error as _;
                    use std::fmt::Write as _;

                    // Buffer the content so we can try deserializing it multiple ways
                    let content = deserializer.deserialize_any(schematic::serde_content::ValueVisitor)?;

                    let mut errors: Vec<(&str, String)> = Vec::new();

                    #(#variant_attempts)*

                    // All variants failed, build the combined error message
                    let mut error_msg = format!("failed to parse as any variant of {}:", stringify!(#partial_name));
                    for (variant_name, error) in &errors {
                        let _ = write!(error_msg, "\n- {}: {}", variant_name, error);
                    }

                    Err(D::Error::custom(error_msg))
                }
            }
        }
    }

    #[cfg(feature = "schema")]
    pub fn generate_partial_schema(config_name: &Ident, generics: &Generics) -> TokenStream {
        let (_, ty_generics, _) = generics.split_for_impl();

        quote! {
            let mut schema = <#config_name #ty_generics as schematic::Schematic>::build_schema(schema);
            schematic::internal::partialize_schema(&mut schema, true);
        }
    }
}

/// Build one deserialize-attempt block for the untagged enum's manual
/// `Deserialize` impl, matching the variant's field shape.
fn build_variant_attempt(
    variant: &crate::common::Variant<'_>,
    partial_name: &Ident,
) -> TokenStream {
    use syn::Fields;

    let name = &variant.name;
    let variant_name_str = variant.get_name(Some(&variant.casing_format));

    let wrap_inner = |ty: &syn::Type| -> TokenStream {
        if variant.is_nested() {
            quote! { <#ty as schematic::Config>::Partial }
        } else {
            quote! { #ty }
        }
    };

    match &variant.value.fields {
        Fields::Named(_) => unreachable!(),
        Fields::Unnamed(fields) => {
            let field_types: Vec<_> = fields.unnamed.iter().map(|f| &f.ty).collect();

            if field_types.len() == 1 {
                let inner_ty = wrap_inner(field_types[0]);

                quote! {
                    {
                        let deserializer = schematic::serde_content::Deserializer::new(content.clone())
                            .coerce_numbers()
                            .human_readable();
                        match <#inner_ty as serde::Deserialize>::deserialize(deserializer) {
                            Ok(value) => return Ok(#partial_name::#name(value)),
                            Err(e) => errors.push((#variant_name_str, e.to_string())),
                        }
                    }
                }
            } else {
                let inner_types: Vec<TokenStream> =
                    field_types.iter().map(|ty| wrap_inner(ty)).collect();
                let field_accessors: Vec<TokenStream> = (0..field_types.len())
                    .map(|i| {
                        let idx = syn::Index::from(i);
                        quote! { value.#idx }
                    })
                    .collect();

                quote! {
                    {
                        let deserializer = schematic::serde_content::Deserializer::new(content.clone())
                            .coerce_numbers()
                            .human_readable();
                        match <(#(#inner_types),*) as serde::Deserialize>::deserialize(deserializer) {
                            Ok(value) => return Ok(#partial_name::#name(#(#field_accessors),*)),
                            Err(e) => errors.push((#variant_name_str, e.to_string())),
                        }
                    }
                }
            }
        }
        Fields::Unit => quote! {
            {
                let deserializer = schematic::serde_content::Deserializer::new(content.clone())
                    .coerce_numbers()
                    .human_readable();
                match <() as serde::Deserialize>::deserialize(deserializer) {
                    Ok(_) => return Ok(#partial_name::#name),
                    Err(e) => errors.push((#variant_name_str, e.to_string())),
                }
            }
        },
    }
}
