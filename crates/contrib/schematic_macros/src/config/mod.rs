pub mod container;
pub mod field;
pub mod field_value;
pub mod variant;

use proc_macro2::TokenStream;
use quote::{ToTokens, format_ident, quote};
use syn::{Generics, parse_quote};

use crate::{common::Macro, utils::instrument_quote};

pub struct ConfigMacro<'l>(pub Macro<'l>);

/// Extend `generics` with predicates from `predicate_factory` applied to every
/// type parameter of `base_generics`, then return the cloned `Generics`.
fn extend_with_predicates<F>(base_generics: &Generics, predicate_factory: F) -> Generics
where
    F: Fn(&syn::Ident) -> syn::WherePredicate,
{
    let mut generics = base_generics.clone();
    let wc = generics.make_where_clause();
    for tp in base_generics.type_params() {
        wc.predicates.push(predicate_factory(&tp.ident));
    }
    generics
}

impl ToTokens for ConfigMacro<'_> {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        let cfg = &self.0;
        let name = cfg.name;

        let serde = quote! { ::schematic::serde };

        let generics = extend_with_predicates(
            cfg.generics,
            |ident| parse_quote!(#ident: Clone + std::fmt::Debug + PartialEq + #serde::Serialize + #serde::de::DeserializeOwned),
        );
        let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();

        let schematic_generics = extend_with_predicates(
            cfg.generics,
            |ident| parse_quote!(#ident: schematic::Schematic),
        );
        let schematic_where = schematic_generics.where_clause.as_ref();

        let partial_schematic_generics = extend_with_predicates(
            cfg.generics,
            |ident| parse_quote!(#ident: Clone + PartialEq + #serde::Serialize + #serde::de::DeserializeOwned + schematic::Schematic),
        );
        let partial_schematic_where = partial_schematic_generics.where_clause.as_ref();

        let partial_config_generics = extend_with_predicates(
            cfg.generics,
            |ident| parse_quote!(#ident: Clone + PartialEq + #serde::Serialize + #serde::de::DeserializeOwned + schematic::Schematic),
        );
        let partial_config_where = partial_config_generics.where_clause.as_ref();

        // Generate the partial implementation
        let partial_name = format_ident!("Partial{}", cfg.name);
        let partial_attrs = cfg.get_partial_attrs();
        let is_untagged = cfg.is_untagged();
        let partial = cfg.type_of.generate_partial(
            &partial_name,
            &partial_attrs,
            cfg.generics,
            !cfg.args.no_deserialize_derive,
            is_untagged,
        );
        let partial_default_impl = cfg.type_of.generate_partial_default_impl(
            &partial_name,
            cfg.generics,
            !cfg.args.no_deserialize_derive,
            is_untagged,
        );

        tokens.extend(quote! {
            #partial
            #partial_default_impl
        });

        let instrument = instrument_quote();
        tokens.extend(emit_config_impls(&ConfigImplArgs {
            cfg,
            name,
            partial_name: &partial_name,
            impl_generics: &impl_generics,
            ty_generics: &ty_generics,
            partial_config_where,
            instrument: &instrument,
        }));

        if cfg.args.default {
            tokens.extend(emit_default_impl(
                name,
                &impl_generics,
                &ty_generics,
                where_clause,
                &instrument,
            ));
        }

        #[cfg(feature = "schema")]
        tokens.extend(emit_schematic_impls(&SchematicImplArgs {
            cfg,
            name,
            partial_name: &partial_name,
            impl_generics: &impl_generics,
            ty_generics: &ty_generics,
            schematic_where,
            partial_schematic_where,
            instrument: &instrument,
        }));

        #[cfg(not(feature = "schema"))]
        tokens.extend(emit_minimal_schematic_impls(
            name,
            &partial_name,
            &impl_generics,
            &ty_generics,
            where_clause,
        ));
    }
}

#[cfg(feature = "schema")]
struct SchematicImplArgs<'a> {
    cfg: &'a Macro<'a>,
    name: &'a syn::Ident,
    partial_name: &'a syn::Ident,
    impl_generics: &'a syn::ImplGenerics<'a>,
    ty_generics: &'a syn::TypeGenerics<'a>,
    schematic_where: Option<&'a syn::WhereClause>,
    partial_schematic_where: Option<&'a syn::WhereClause>,
    instrument: &'a TokenStream,
}

#[cfg(feature = "schema")]
fn emit_schematic_impls(args: &SchematicImplArgs<'_>) -> TokenStream {
    let &SchematicImplArgs {
        cfg,
        name,
        partial_name,
        impl_generics,
        ty_generics,
        schematic_where,
        partial_schematic_where,
        instrument,
    } = args;

    let schema_name = cfg.get_name();
    let schema_impl = cfg.type_of.generate_schema(&cfg.attrs);
    let partial_schema_name = partial_name.to_string();
    let partial_schema_impl = crate::common::Container::generate_partial_schema(name, cfg.generics);

    quote! {
        #[automatically_derived]
        impl #impl_generics schematic::Schematic for #name #ty_generics #schematic_where {
            fn schema_name() -> Option<String> {
                Some(#schema_name.into())
            }

            #instrument
            fn build_schema(mut schema: schematic::SchemaBuilder) -> schematic::Schema {
                use schematic::schema::*;

                #schema_impl
            }
        }

        #[automatically_derived]
        impl #impl_generics schematic::Schematic for #partial_name #ty_generics #partial_schematic_where {
            fn schema_name() -> Option<String> {
                Some(#partial_schema_name.into())
            }

            #instrument
            fn build_schema(mut schema: schematic::SchemaBuilder) -> schematic::Schema {
                #partial_schema_impl
                schema
            }
        }
    }
}

#[cfg(not(feature = "schema"))]
fn emit_minimal_schematic_impls(
    name: &syn::Ident,
    partial_name: &syn::Ident,
    impl_generics: &syn::ImplGenerics<'_>,
    ty_generics: &syn::TypeGenerics<'_>,
    where_clause: Option<&syn::WhereClause>,
) -> TokenStream {
    quote! {
        #[automatically_derived]
        impl #impl_generics schematic::Schematic for #name #ty_generics #where_clause {}

        #[automatically_derived]
        impl #impl_generics schematic::Schematic for #partial_name #ty_generics #where_clause {}
    }
}

struct ConfigImplArgs<'a> {
    cfg: &'a Macro<'a>,
    name: &'a syn::Ident,
    partial_name: &'a syn::Ident,
    impl_generics: &'a syn::ImplGenerics<'a>,
    ty_generics: &'a syn::TypeGenerics<'a>,
    partial_config_where: Option<&'a syn::WhereClause>,
    instrument: &'a TokenStream,
}

fn emit_config_impls(args: &ConfigImplArgs<'_>) -> TokenStream {
    let &ConfigImplArgs {
        cfg,
        name,
        partial_name,
        impl_generics,
        ty_generics,
        partial_config_where,
        instrument,
    } = args;

    let default_values = cfg.type_of.generate_default_values();
    let empty_values = cfg.type_of.generate_empty_values();
    let finalize = cfg.type_of.generate_finalize();
    let merge = cfg.type_of.generate_merge();
    let from_partial = cfg.type_of.generate_from_partial(partial_name);
    let settings_metadata = cfg.type_of.generate_settings_metadata();

    let is_empty_impl = if let Some(func) = &cfg.args.is_empty {
        quote! { #func(self) }
    } else {
        cfg.type_of.generate_is_empty_impl()
    };

    let context = if let Some(ctx) = cfg.args.context.as_ref() {
        quote! { #ctx }
    } else {
        quote! { () }
    };

    let env_method = if cfg!(feature = "env") {
        let env_values = cfg.type_of.generate_env_values();

        quote! {
            #instrument
            fn env_values() -> std::result::Result<Option<Self>, schematic::ConfigError> {
                use schematic::internal::*;
                #env_values
            }
        }
    } else {
        quote! {}
    };

    quote! {
        #[automatically_derived]
        impl #impl_generics schematic::PartialConfig for #partial_name #ty_generics #partial_config_where {
            type Context = #context;

            #instrument
            fn empty() -> Self {
                #empty_values
            }

            fn is_empty(&self) -> bool {
                #is_empty_impl
            }

            #instrument
            fn default_values(context: &Self::Context) -> std::result::Result<Option<Self>, schematic::ConfigError> {
                use schematic::internal::*;
                #default_values
            }

            #env_method

            #instrument
            fn finalize(self, context: &Self::Context) -> std::result::Result<Self, schematic::ConfigError> {
                #finalize
            }

            #instrument
            fn merge(
                &mut self,
                context: &Self::Context,
                mut next: Self,
            ) -> std::result::Result<(), schematic::ConfigError> {
                use schematic::internal::*;
                #merge
            }
        }

        #[automatically_derived]
        impl #impl_generics schematic::Config for #name #ty_generics #partial_config_where {
            type Partial = #partial_name #ty_generics;

            #instrument
            fn from_partial(partial: Self::Partial, fields: Vec<String>) -> std::result::Result<Self, schematic::ConfigError> {
                #from_partial
            }

            #instrument
            fn settings() -> schematic::ConfigSettingMap {
                #settings_metadata
            }
        }
    }
}

fn emit_default_impl(
    name: &syn::Ident,
    impl_generics: &syn::ImplGenerics<'_>,
    ty_generics: &syn::TypeGenerics<'_>,
    where_clause: Option<&syn::WhereClause>,
    instrument: &TokenStream,
) -> TokenStream {
    quote! {
        #[automatically_derived]
        impl #impl_generics Default for #name #ty_generics #where_clause {
            #instrument
            fn default() -> Self {
                let context = <<Self as schematic::Config>::Partial as schematic::PartialConfig>::Context::default();

                let defaults = <<Self as schematic::Config>::Partial as schematic::PartialConfig>::default_values(&context).unwrap().unwrap_or_default();

                <Self as schematic::Config>::from_partial(defaults, vec![])
                    .expect("any partial with missing required values will not derive Default")
            }
        }
    }
}
