use std::collections::HashSet;

use darling::FromAttributes;
use proc_macro2::{Ident, TokenStream};
use quote::{ToTokens, quote};
use syn::{Attribute, Expr, ExprPath, Field as NativeField, Type};

use crate::{
    common::{FieldValue, PartialAttr, TypeInfo, extract_inner_type, macros::ContainerSerdeArgs},
    utils::{DefaultAttr, extract_common_attrs, format_case, parse_default},
};

// #[serde()]
#[derive(FromAttributes, Default, Debug)]
#[darling(default, allow_unknown_fields, attributes(serde))]
pub struct FieldSerdeArgs {
    pub alias: Option<String>,
    pub default: bool,
    pub deserialize_with: Option<String>,
    pub flatten: bool,
    pub rename: Option<String>,
    pub skip: bool,
    pub skip_deserializing: bool,
    pub skip_serializing: bool,

    // variant
    pub untagged: bool,
}

impl FieldSerdeArgs {
    pub fn inherit_from_container(&mut self, container: &ContainerSerdeArgs) {
        if !self.default && container.default {
            self.default = true;
        }
    }
}

// #[schema()], #[setting()]
#[derive(FromAttributes, Default, Debug)]
#[darling(default, attributes(schema, setting))]
pub struct FieldArgs {
    // schema
    pub exclude: bool,

    // config
    #[darling(with = parse_default)]
    pub default: DefaultAttr,
    #[cfg(feature = "env")]
    pub env: Option<String>,
    pub merge: Option<ExprPath>,
    pub nested: bool,
    #[cfg(feature = "env")]
    pub parse_env: Option<ExprPath>,
    pub required: bool,
    pub transform: Option<ExprPath>,
    pub partial: PartialAttr,
    pub is_empty: Option<ExprPath>,
    pub partial_via: Option<ExprPath>,

    // Declare input shapes the field's type does not describe, for a field
    // whose `deserialize_with` accepts more than the type alone would.
    //
    // Names a `fn(&SchemaBuilder) -> Vec<Schema>`. The inferred schema is
    // unioned with the returned variants and marked as the expanded form, so
    // the field's own keys stay addressable alongside the extra shapes.
    pub schema_union_with: Option<ExprPath>,

    // serde
    pub alias: Option<String>,
    pub deserialize_with: Option<String>,
    pub flatten: bool,
    pub rename: Option<String>,
    pub skip: bool,
    pub skip_deserializing: bool,
    pub skip_serializing: bool,
    pub skip_serializing_if: Option<String>,
}

#[derive(Debug)]
pub struct Field<'l> {
    pub args: FieldArgs,
    pub serde_args: FieldSerdeArgs,
    pub attrs: Vec<&'l Attribute>,
    pub casing_format: String,
    pub name: Option<&'l Ident>, // Named
    pub index: usize,            // Unnamed
    pub value: &'l Type,
    pub value_type: FieldValue,
    pub env_prefix: Option<String>,
    pub partial_via_ty: Option<Type>, // owns the via type so FieldValue can borrow it
}

impl Field<'_> {
    pub fn from(field: &NativeField) -> Field<'_> {
        // Errors are fatal rather than defaulted: a mistyped or unknown key in
        // schematic's own `#[setting]` / `#[schema]` attribute would otherwise
        // silently discard every other key on the same field.
        //
        // `#[serde]` stays lenient. It is serde's namespace, not schematic's,
        // and carries keys (`with`, `bound`, `default = "path"`) that
        // `FieldSerdeArgs` deliberately does not model.
        let args = FieldArgs::from_attributes(&field.attrs).unwrap_or_else(|error| {
            let name = field
                .ident
                .as_ref()
                .map_or_else(|| "<unnamed>".to_owned(), ToString::to_string);

            panic!("Invalid `#[setting]` or `#[schema]` attribute on field `{name}`: {error}");
        });
        let serde_args = FieldSerdeArgs::from_attributes(&field.attrs).unwrap_or_default();

        let partial_via_ty = args.partial_via.as_ref().map(|ep| {
            Type::Path(syn::TypePath {
                qself: ep.qself.clone(),
                path: ep.path.clone(),
            })
        });

        // Can't construct value_type yet — need to borrow from partial_via_ty
        // after it's stored in the struct. Build a temporary Field first, then
        // set value_type.

        let mut result = Field {
            name: field.ident.as_ref(),
            index: 0,
            attrs: extract_common_attrs(&field.attrs),
            casing_format: String::new(),
            value: &field.ty,
            value_type: FieldValue::value(&field.ty), // placeholder
            args,
            serde_args,
            env_prefix: None,
            partial_via_ty,
        };

        result.value_type = if result.args.nested {
            let raw_ty = result.partial_via_ty.as_ref().unwrap_or(result.value);
            let mut value_type = FieldValue::nested(raw_ty);

            if result.partial_via_ty.is_some() {
                let mut field_info = TypeInfo::default();
                extract_inner_type(result.value, &mut field_info);
                match &mut value_type {
                    FieldValue::NestedValue { info, .. } => {
                        info.optional = field_info.optional;
                        info.boxed = field_info.boxed;
                    }
                    FieldValue::NestedList {
                        collection_info, ..
                    }
                    | FieldValue::NestedMap {
                        collection_info, ..
                    } => {
                        collection_info.optional = field_info.optional;
                        collection_info.boxed = field_info.boxed;
                    }
                    FieldValue::Value { .. } => {}
                }
            }
            value_type
        } else {
            FieldValue::value(result.value)
        };

        result
    }

    #[cfg(feature = "schema")]
    pub fn is_excluded(&self) -> bool {
        self.args.exclude
    }

    #[cfg(feature = "schema")]
    pub fn is_flatten(&self) -> bool {
        self.serde_args.flatten || self.args.flatten
    }

    pub fn is_nested(&self) -> bool {
        self.args.nested
    }

    pub fn is_nullable(&self) -> bool {
        self.value_type.is_outer_optional()
    }

    pub fn is_container(&self) -> bool {
        self.value_type.is_container()
    }

    #[cfg(feature = "schema")]
    pub fn is_optional(&self) -> bool {
        self.serde_args.default || self.args.default.is_declared()
    }

    pub fn is_required(&self) -> bool {
        self.args.required
    }

    #[cfg(feature = "schema")]
    pub fn is_skipped(&self) -> bool {
        self.args.skip || self.serde_args.skip
    }

    pub fn get_name(&self, casing_format: Option<&str>) -> String {
        let Some(name) = &self.name else {
            return String::new();
        };

        match &self.args.rename {
            Some(local) => local.to_owned(),
            _ => {
                if let Some(serde) = &self.serde_args.rename {
                    serde.to_owned()
                } else if let Some(format) = casing_format {
                    format_case(format, &name.to_string(), false)
                } else {
                    name.to_string()
                }
            }
        }
    }

    #[cfg(feature = "schema")]
    pub fn get_aliases(&self) -> Vec<String> {
        let mut aliases = HashSet::new();

        if let Some(alias) = &self.args.alias {
            aliases.insert(alias.to_owned());
        }

        if let Some(alias) = &self.serde_args.alias {
            aliases.insert(alias.to_owned());
        }

        aliases.into_iter().collect()
    }

    pub fn get_env_var(&self) -> Option<String> {
        if self.is_nested() {
            return None;
        }

        #[cfg(feature = "env")]
        if let Some(env_name) = &self.args.env {
            return Some(env_name.to_owned());
        }

        self.env_prefix
            .as_ref()
            .map(|env_prefix| format!("{env_prefix}{}", self.get_name(None)).to_uppercase())
    }

    pub fn get_serde_meta(&self) -> Option<TokenStream> {
        let mut meta = vec![];

        match &self.args.alias {
            Some(alias) => {
                meta.push(quote! { alias = #alias });
            }
            _ => {
                if let Some(alias) = &self.serde_args.alias {
                    meta.push(quote! { alias = #alias });
                }
            }
        }

        if self.args.flatten || self.serde_args.flatten {
            meta.push(quote! { flatten });
        }

        match &self.args.rename {
            Some(rename) => {
                meta.push(quote! { rename = #rename });
            }
            _ => {
                if let Some(rename) = &self.serde_args.rename {
                    meta.push(quote! { rename = #rename });
                }
            }
        }

        let mut skipped = false;

        if self.args.skip || self.serde_args.skip {
            meta.push(quote! { skip });
            skipped = true;
        }

        if !skipped {
            if let Some(skip_serializing_if) = self.args.skip_serializing_if.clone() {
                meta.push(quote! {
                skip_serializing_if = #skip_serializing_if });
            } else if self.args.skip_serializing || self.serde_args.skip_serializing {
                meta.push(quote! { skip_serializing });
            } else {
                let tokens = match &self.value_type {
                    FieldValue::NestedMap {
                        collection,
                        collection_info,
                        ..
                    }
                    | FieldValue::NestedList {
                        collection,
                        collection_info,
                        ..
                    } => {
                        if collection_info.optional {
                            quote! { skip_serializing_if = "Option::is_none" }
                        } else {
                            let name = collection.to_string();
                            let fmt = format!("{name}::is_empty");
                            quote! { skip_serializing_if = #fmt }
                        }
                    }
                    FieldValue::NestedValue { info, .. } => {
                        if info.optional {
                            quote! { skip_serializing_if = "Option::is_none" }
                        } else {
                            quote! { skip_serializing_if = "::schematic::PartialConfig::is_empty" }
                        }
                    }
                    FieldValue::Value { .. } => {
                        quote! { skip_serializing_if = "Option::is_none" }
                    }
                };

                meta.push(tokens);
            }

            if self.args.skip_deserializing || self.serde_args.skip_deserializing {
                meta.push(quote! { skip_deserializing });
            } else if let Some(deserialize_with) = self
                .args
                .deserialize_with
                .as_ref()
                .or(self.serde_args.deserialize_with.as_ref())
            {
                // A field reads the same way in both forms, so the partial
                // takes the custom deserializer whichever namespace declared
                // it. Reading only `#[setting]` here would leave a field that
                // used `#[serde]` parsing one way as a resolved config and
                // another as a layer.
                //
                // `deserialize_with` disables serde's implicit "missing
                // `Option` field is `None`" handling, so the partial field
                // needs an explicit default to stay optional.
                meta.push(quote! { default });
                meta.push(quote! { deserialize_with = #deserialize_with });
            }
        }

        if meta.is_empty() {
            return None;
        }

        Some(quote! {
            #(#meta),*
        })
    }

    #[cfg(feature = "schema")]
    pub fn generate_schema_type(&self, as_field: bool) -> TokenStream {
        use syn::Lit;

        use crate::utils::{
            extract_comment, extract_deprecated, map_bool_field_quote, map_option_field_quote,
            map_vec_field_quote,
        };

        let aliases = map_vec_field_quote("aliases", &self.get_aliases());
        let hidden = map_bool_field_quote("hidden", self.is_skipped());
        let flatten = map_bool_field_quote("flatten", self.is_flatten());
        let nullable = map_bool_field_quote("nullable", self.is_nullable());
        let optional = map_bool_field_quote("optional", self.is_optional());
        let comment = map_option_field_quote("comment", extract_comment(&self.attrs));
        let description = map_option_field_quote("description", extract_comment(&self.attrs));
        let deprecated = map_option_field_quote("deprecated", extract_deprecated(&self.attrs));
        let env_var = map_option_field_quote("env_var", self.get_env_var());

        // A `partial_via` field is written in the shape of its via type, not of
        // the type it resolves to: `attachments` holds a `Vec<AttachmentConfig>`
        // once resolved, but a document writes either a bare list or the merge
        // wrapper carrying one, and the via type is what describes both.
        let value = if self.partial_via_ty.is_some() && self.is_container() {
            // Nested collections deserialize partial elements in every wire form.
            self.value_type.to_token_stream()
        } else {
            self.partial_via_ty
                .as_ref()
                .unwrap_or(self.value)
                .to_token_stream()
        };
        let mut inner_schema = if self.is_nested() {
            quote! { schema.infer_as_nested::<#value>() }
        } else {
            quote! { schema.infer::<#value>() }
        };

        if let Some(Expr::Lit(lit)) = self.args.default.expr() {
            let lit_value = match &lit.lit {
                Lit::Str(v) => quote! { LiteralValue::String(#v.into()) },
                Lit::Int(v) => {
                    if v.suffix().starts_with('u') {
                        quote! { LiteralValue::UInt(#v) }
                    } else {
                        quote! { LiteralValue::Int(#v) }
                    }
                }
                Lit::Float(v) => {
                    if v.suffix() == "f32" {
                        quote! { LiteralValue::F32(#v) }
                    } else {
                        quote! { LiteralValue::F64(#v) }
                    }
                }
                Lit::Bool(v) => quote! { LiteralValue::Bool(#v) },
                _ => unimplemented!(),
            };

            inner_schema = quote! { schema.infer_with_default::<#value>(#lit_value) };
        }

        if let Some(path) = &self.args.schema_union_with {
            inner_schema = union_with_declared_shapes(path, &inner_schema);
        }

        // Struct field (named)
        if as_field {
            let name = self.get_name(Some(&self.casing_format));
            let value = if aliases.is_none()
                && comment.is_none()
                && deprecated.is_none()
                && env_var.is_none()
                && flatten.is_none()
                && hidden.is_none()
                && nullable.is_none()
                && optional.is_none()
            {
                quote! {
                    SchemaField::new(#inner_schema)
                }
            } else {
                quote! {
                    {
                        let mut field = SchemaField::new(#inner_schema);
                        #aliases
                        #comment
                        #deprecated
                        #env_var
                        #flatten
                        #hidden
                        #nullable
                        #optional
                        field
                    }
                }
            };

            quote! {
                (#name.into(), #value)
            }
        }
        // Tuple item (unnamed)
        else {
            #[allow(clippy::collapsible_else_if)]
            if description.is_none() {
                inner_schema
            } else {
                quote! {
                    {
                        let mut schema = #inner_schema;
                        #description
                        schema
                    }
                }
            }
        }
    }
}

/// Union a field's inferred schema with shapes its type cannot describe.
///
/// The inferred schema goes last and is marked as the expanded form: the
/// declared variants are shorthand spellings of it, so a consumer resolving
/// keys follows the inferred fields.
#[cfg(feature = "schema")]
fn union_with_declared_shapes(path: &ExprPath, inner_schema: &TokenStream) -> TokenStream {
    quote! {
        {
            let described = #inner_schema;
            let mut variants = #path(&schema);

            let expanded = variants.len();
            variants.push(described);

            let mut nested = schema.nest();
            nested.union(UnionType::new_any(variants).with_expanded_index(expanded))
        }
    }
}

impl ToTokens for Field<'_> {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        let value = &self.value_type;

        // Gather all attributes
        let mut attrs = vec![];

        if let Some(serde_meta) = self.get_serde_meta() {
            attrs.push(quote! { #[serde(#serde_meta)] });
        }

        for attr in &self.attrs {
            attrs.push(quote! { #attr });
        }
        let partial = &self.args.partial;
        attrs.push(quote! {#partial});

        if let Some(name) = &self.name {
            tokens.extend(quote! {
                #(#attrs)*
                pub #name: #value,
            });
        } else {
            tokens.extend(quote! {
                pub #value,
            });
        }
    }
}
