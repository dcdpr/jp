//! Naming the item a line of Rust source sits in.
//!
//! A finding's symbol is the path of the innermost named item spanning its
//! line, relative to the file: `parse`, `Parser::run`, `<Parser as
//! Display>::fmt`, `tests::reads_input`.
//! The module path is left out, so moving a file changes no symbol in it.
//! Generic parameters are left out too, so they can change without changing the
//! symbol.

use proc_macro2::Span;
use syn::{
    ImplItemConst, ImplItemFn, ImplItemType, ItemConst, ItemEnum, ItemFn, ItemImpl, ItemMacro,
    ItemMod, ItemStatic, ItemStruct, ItemTrait, ItemType, ItemUnion, Path, TraitItemFn, Type,
    spanned::Spanned,
    visit::{self, Visit},
};

/// Every named item in one file, with the lines it spans.
#[derive(Debug, Default)]
pub struct Symbols {
    scopes: Vec<Scope>,
}

#[derive(Debug)]
struct Scope {
    start: usize,
    end: usize,
    depth: usize,
    path: String,
}

impl Symbols {
    /// Index the items in `source`.
    ///
    /// # Errors
    ///
    /// Returns an error when `source` isn't valid Rust.
    pub fn parse(source: &str) -> syn::Result<Self> {
        let file = syn::parse_file(source)?;
        let mut collector = Collector::default();
        collector.visit_file(&file);

        Ok(Self {
            scopes: collector.scopes,
        })
    }

    /// The path of the innermost named item spanning the 1-based `line`.
    ///
    /// An item spans its attributes, doc comments included.
    /// Empty when no item does: a `use`, a plain `//` comment between items, or
    /// a line past the end.
    #[must_use]
    pub fn at(&self, line: usize) -> String {
        self.scopes
            .iter()
            .filter(|scope| scope.start <= line && line <= scope.end)
            .max_by_key(|scope| scope.depth)
            .map(|scope| scope.path.clone())
            .unwrap_or_default()
    }
}

#[derive(Default)]
struct Collector {
    stack: Vec<String>,
    scopes: Vec<Scope>,
}

impl Collector {
    fn enter(&mut self, name: String, span: Span, walk: impl FnOnce(&mut Self)) {
        self.stack.push(name);
        self.scopes.push(Scope {
            start: span.start().line,
            end: span.end().line,
            depth: self.stack.len(),
            path: self.stack.join("::"),
        });
        walk(self);
        self.stack.pop();
    }
}

impl<'ast> Visit<'ast> for Collector {
    fn visit_item_fn(&mut self, item: &'ast ItemFn) {
        self.enter(item.sig.ident.to_string(), item.span(), |c| {
            visit::visit_item_fn(c, item);
        });
    }

    fn visit_item_impl(&mut self, item: &'ast ItemImpl) {
        self.enter(impl_name(item), item.span(), |c| {
            visit::visit_item_impl(c, item);
        });
    }

    fn visit_impl_item_fn(&mut self, item: &'ast ImplItemFn) {
        self.enter(item.sig.ident.to_string(), item.span(), |c| {
            visit::visit_impl_item_fn(c, item);
        });
    }

    fn visit_impl_item_const(&mut self, item: &'ast ImplItemConst) {
        self.enter(item.ident.to_string(), item.span(), |c| {
            visit::visit_impl_item_const(c, item);
        });
    }

    fn visit_impl_item_type(&mut self, item: &'ast ImplItemType) {
        self.enter(item.ident.to_string(), item.span(), |c| {
            visit::visit_impl_item_type(c, item);
        });
    }

    fn visit_item_trait(&mut self, item: &'ast ItemTrait) {
        self.enter(item.ident.to_string(), item.span(), |c| {
            visit::visit_item_trait(c, item);
        });
    }

    fn visit_trait_item_fn(&mut self, item: &'ast TraitItemFn) {
        self.enter(item.sig.ident.to_string(), item.span(), |c| {
            visit::visit_trait_item_fn(c, item);
        });
    }

    fn visit_item_mod(&mut self, item: &'ast ItemMod) {
        self.enter(item.ident.to_string(), item.span(), |c| {
            visit::visit_item_mod(c, item);
        });
    }

    fn visit_item_struct(&mut self, item: &'ast ItemStruct) {
        self.enter(item.ident.to_string(), item.span(), |c| {
            visit::visit_item_struct(c, item);
        });
    }

    fn visit_item_enum(&mut self, item: &'ast ItemEnum) {
        self.enter(item.ident.to_string(), item.span(), |c| {
            visit::visit_item_enum(c, item);
        });
    }

    fn visit_item_union(&mut self, item: &'ast ItemUnion) {
        self.enter(item.ident.to_string(), item.span(), |c| {
            visit::visit_item_union(c, item);
        });
    }

    fn visit_item_const(&mut self, item: &'ast ItemConst) {
        self.enter(item.ident.to_string(), item.span(), |c| {
            visit::visit_item_const(c, item);
        });
    }

    fn visit_item_static(&mut self, item: &'ast ItemStatic) {
        self.enter(item.ident.to_string(), item.span(), |c| {
            visit::visit_item_static(c, item);
        });
    }

    fn visit_item_type(&mut self, item: &'ast ItemType) {
        self.enter(item.ident.to_string(), item.span(), |c| {
            visit::visit_item_type(c, item);
        });
    }

    // `macro_rules! name` is the only item macro with a name of its own.
    fn visit_item_macro(&mut self, item: &'ast ItemMacro) {
        match &item.ident {
            Some(ident) => self.enter(ident.to_string(), item.span(), |c| {
                visit::visit_item_macro(c, item);
            }),
            None => visit::visit_item_macro(self, item),
        }
    }
}

/// `Type` for an inherent impl, `<Type as Trait>` for a trait impl.
///
/// The trait is part of the name because two trait impls on one type often
/// define a method with the same name, `fmt` for `Display` and `Debug`.
fn impl_name(item: &ItemImpl) -> String {
    let self_ty = type_name(&item.self_ty);
    match &item.trait_ {
        Some((_, path, _)) => format!("<{self_ty} as {}>", last_segment(path)),
        None => self_ty,
    }
}

fn type_name(ty: &Type) -> String {
    match ty {
        Type::Path(path) => last_segment(&path.path),
        Type::Reference(reference) => type_name(&reference.elem),
        _ => "_".to_owned(),
    }
}

fn last_segment(path: &Path) -> String {
    path.segments
        .last()
        .map(|segment| segment.ident.to_string())
        .unwrap_or_default()
}

#[cfg(test)]
#[path = "symbol_tests.rs"]
mod tests;
