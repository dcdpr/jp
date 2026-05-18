use proc_macro2::{Ident, TokenStream};
use quote::{format_ident, quote};
use syn::{Expr, Fields, FieldsUnnamed, Lit};

use crate::{common::Variant, utils::expr_path_with_turbofish};

impl Variant<'_> {
    pub fn generate_default_value(&self) -> TokenStream {
        let name = &self.name;

        match &self.value.fields {
            Fields::Named(_) => unreachable!(),
            Fields::Unnamed(fields) => {
                let fields = fields
                    .unnamed
                    .iter()
                    .map(|_| {
                        quote! { Default::default() }
                    })
                    .collect::<Vec<_>>();

                quote! { #name(#(#fields),*) }
            }
            Fields::Unit => quote! { #name },
        }
    }

    pub fn generate_is_empty(&self) -> TokenStream {
        let name = &self.name;

        match &self.value.fields {
            Fields::Named(_) => unreachable!(),
            Fields::Unnamed(fields) => Self::map_unnamed_match(self.name, fields, |idents, _| {
                let stmts = idents
                    .iter()
                    .enumerate()
                    .map(|(i, ident)| {
                        let ty = &fields.unnamed[i].ty;
                        if let Some(expr) = &self.args.is_empty.as_ref() {
                            match expr {
                                Expr::Array(_)
                                | Expr::Call(_)
                                | Expr::Macro(_)
                                | Expr::Tuple(_) => quote! { #expr },
                                Expr::Path(func) => quote! { #func(#ident) },
                                Expr::Lit(lit) => match &lit.lit {
                                    Lit::Str(string) => {
                                        let ty = expr_path_with_turbofish(ty);
                                        quote! { #ty::try_from(#string) }
                                    }
                                    other => quote! { #other },
                                },
                                v => {
                                    let v = format!("{v:?}");
                                    panic!(
                                        "Unsupported `is_empty` value ({v}). May only provide \
                                         paths, literals, primitives, arrays, or tuples."
                                    )
                                }
                            }
                        } else if self.is_nested() {
                            quote! { #ident.is_empty() }
                        } else {
                            quote! { #ident == &<#ty as Default>::default() }
                        }
                    })
                    .collect::<Vec<_>>();

                quote! {
                    #(#stmts) && *
                }
            }),
            Fields::Unit => {
                let value = if let Some(expr) = &self.args.is_empty.as_ref() {
                    match expr {
                        Expr::Array(_)
                        | Expr::Call(_)
                        | Expr::Macro(_)
                        | Expr::Tuple(_)
                        | Expr::Lit(_) => quote! { #expr },
                        Expr::Path(func) => quote! { #func() },
                        v => {
                            let v = format!("{v:?}");
                            panic!(
                                "Unsupported `is_empty` value ({v}). May only provide paths, \
                                 literals, primitives, arrays, or tuples."
                            )
                        }
                    }
                } else {
                    quote! { false }
                };

                quote! { Self::#name => #value, }
            }
        }
    }

    pub fn generate_empty_value(&self) -> TokenStream {
        let name = &self.name;

        match &self.value.fields {
            Fields::Named(_) => unreachable!(),
            Fields::Unnamed(fields) => {
                let fields = fields
                    .unnamed
                    .iter()
                    .map(|v| {
                        if self.is_nested() {
                            let ty = &v.ty;
                            quote! { <#ty as schematic::Config>::Partial::empty() }
                        } else {
                            quote! { Default::default() }
                        }
                    })
                    .collect::<Vec<_>>();

                quote! { #name(#(#fields),*) }
            }
            Fields::Unit => quote! { #name },
        }
    }

    pub fn generate_finalize_statement(&self) -> Option<TokenStream> {
        let name = &self.name;

        match &self.value.fields {
            Fields::Named(_) => unreachable!(),
            Fields::Unnamed(fields) => {
                if !self.is_nested() {
                    return None;
                }

                Some(Self::map_unnamed_match(
                    self.name,
                    fields,
                    |outer_names, _| {
                        let stmts = outer_names
                            .iter()
                            .map(|o| {
                                quote! { #o.finalize(context)? }
                            })
                            .collect::<Vec<_>>();

                        quote! {
                            Self::#name(#(#stmts),*)
                        }
                    },
                ))
            }
            Fields::Unit => None,
        }
    }

    pub fn generate_merge_statement(&self) -> Option<TokenStream> {
        let name = &self.name;
        let args = &self.args;

        match &self.value.fields {
            Fields::Named(_) => unreachable!(),
            Fields::Unnamed(fields) => {
                if self.is_nested() {
                    assert!(
                        args.merge.is_none(),
                        "Nested variants do not support `merge`."
                    );

                    return Some(Self::map_unnamed_match(
                        self.name,
                        fields,
                        |outer_names, inner_names| {
                            let merge_stmts = outer_names
                                .iter()
                                .enumerate()
                                .map(|(index, o)| {
                                    let i = &inner_names[index];
                                    quote! { #o.merge(context, #i)?; }
                                })
                                .collect::<Vec<_>>();

                            quote! {
                                if let Self::#name(#(#inner_names),*) = next {
                                    #(#merge_stmts)*
                                } else {
                                    *self = next;
                                }
                            }
                        },
                    ));
                }

                if let Some(func) = args.merge.as_ref() {
                    return Some(Self::map_unnamed_match(
                        self.name,
                        fields,
                        |outer_names, inner_names| {
                            if outer_names.len() == 1 {
                                quote! {
                                    if let Self::#name(ai) = next {
                                        *self = Self::#name(
                                            #func(ao.to_owned(), ai, context)?.unwrap_or_default(),
                                        );
                                    } else {
                                        *self = next;
                                    }
                                }
                            } else {
                                let defaults = outer_names
                                    .iter()
                                    .map(|_| {
                                        quote! { Default::default() }
                                    })
                                    .collect::<Vec<_>>();

                                quote! {
                                    if let Self::#name(#(#inner_names),*) = next {
                                        if let Some((#(#outer_names),*)) = #func(
                                            (#(#outer_names.to_owned()),*),
                                            (#(#inner_names),*),
                                            context,
                                        )? {
                                            *self = Self::#name(#(#outer_names),*);
                                        } else {
                                            *self = Self::#name(#(#defaults),*);
                                        }
                                    } else {
                                        *self = next;
                                    }
                                }
                            }
                        },
                    ));
                }

                None
            }
            Fields::Unit => {
                assert!(
                    args.merge.is_none(),
                    "Unit variants do not support `merge`."
                );

                None
            }
        }
    }

    pub fn generate_from_partial_value(&self, partial_name: &Ident) -> TokenStream {
        let name = &self.name;

        match &self.value.fields {
            Fields::Named(_) => unreachable!(),
            Fields::Unnamed(fields) => {
                Self::map_unnamed_match_custom(self.name, partial_name, fields, |outer_names, _| {
                    let stmts = outer_names
                        .iter()
                        .enumerate()
                        .map(|(index, o)| {
                            if self.is_nested() {
                                let ty = &fields.unnamed[index].ty;
                                let ty = expr_path_with_turbofish(ty);

                                quote! { #ty::from_partial(#o, fields.clone())? }
                            } else {
                                quote! { #o }
                            }
                        })
                        .collect::<Vec<_>>();

                    quote! {
                        Self::#name(#(#stmts),*)
                    }
                })
            }
            Fields::Unit => {
                quote! {
                    #partial_name::#name => Self::#name,
                }
            }
        }
    }

    fn map_unnamed_match<F>(name: &Ident, fields: &FieldsUnnamed, factory: F) -> TokenStream
    where
        F: FnOnce(&[Ident], &[Ident]) -> TokenStream,
    {
        let self_name = format_ident!("Self");

        Self::map_unnamed_match_custom(name, &self_name, fields, factory)
    }

    fn map_unnamed_match_custom<F>(
        name: &Ident,
        self_name: &Ident,
        fields: &FieldsUnnamed,
        factory: F,
    ) -> TokenStream
    where
        F: FnOnce(&[Ident], &[Ident]) -> TokenStream,
    {
        let mut outer_names = vec![];
        let mut inner_names = vec![];

        for (index, _) in fields.unnamed.iter().enumerate() {
            // Index 0 maps to `a`, 1 to `b`, etc.
            let letter = (b'a' + u8::try_from(index).expect("too many unnamed fields")) as char;
            outer_names.push(format_ident!("{}o", letter));
            inner_names.push(format_ident!("{}i", letter));
        }

        let inner = factory(&outer_names, &inner_names);

        quote! {
            #self_name::#name(#(#outer_names),*) => {
                #inner
            },
        }
    }
}
