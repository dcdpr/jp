//! The breaches in one parsed file, and the modules it declares.
//!
//! [`production`] reads a file outside the test tree, [`tests`] one inside it.

use camino::Utf8Path;
use proc_macro2::Span;
use syn::{
    Arm, Attribute, Field, FieldValue, File, ForeignItem, ImplItem, Item, ItemMod, Lit, Local,
    Meta, TraitItem, Variant,
    visit::{self, Visit},
};

use crate::{
    Breach,
    cfg::{self, TestCfg},
    layout::{self, Declaration, TestForm},
};

/// What a file holds: production code, or tests laid out as [`TestForm`] says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Role {
    Production,
    Tests(TestForm),
}

/// The breaches found in a file, each with its line, and the modules the file
/// declares, each with the role its file plays.
#[derive(Debug, Default)]
pub(crate) struct Report {
    pub(crate) breaches: Vec<(usize, Breach)>,
    pub(crate) children: Vec<(Declaration, Role)>,
}

/// Scan a production file at `path`.
///
/// `owns_dir` says whether its child modules live beside it, as for a crate
/// root, a `mod.rs`, or a file loaded through a `#[path]`.
pub(crate) fn production(path: &Utf8Path, owns_dir: bool, file: &File) -> Report {
    let mut scan = Production::default();
    if layout::is_mod_rs(path) {
        scan.report.breaches.push((1, Breach::ModRs {
            rename: layout::mod_rs_rename(path),
        }));
    }

    let (trailing, rest) = split_trailing(&file.items);
    for attr in &file.attrs {
        scan.visit_attribute(attr);
    }
    for item in rest {
        scan.visit_item(item);
    }
    if let Some(module) = trailing {
        scan.trailing(path, owns_dir, module);
    }

    scan.test_functions.report(&mut scan.report);
    scan.report.breaches.sort();
    scan.report
}

/// Scan a test file at `path`, reached as `form`.
pub(crate) fn tests(path: &Utf8Path, form: &TestForm, file: &File) -> Report {
    let mut scan = Tests::default();
    scan.visit_file(file);

    if !layout::holds_test_functions(path) {
        scan.test_functions.report(&mut scan.report);
    }
    match (form, scan.first_declaration) {
        (TestForm::Single { directory }, Some(line)) => {
            scan.report
                .breaches
                .push((line, Breach::SingleFileDeclaresModules {
                    move_to: directory.clone(),
                }));
        }
        (TestForm::Directory { single }, None) => {
            scan.report
                .breaches
                .push((1, Breach::DirectoryWithOneModule {
                    rename: single.clone(),
                }));
        }
        _ => {}
    }

    scan.report.breaches.sort();
    scan.report
}

/// The file's last item when it is a `cfg(test)` module declaration, and the
/// items before it.
fn split_trailing(items: &[Item]) -> (Option<&ItemMod>, &[Item]) {
    match items.split_last() {
        Some((Item::Mod(module), rest))
            if module.content.is_none() && cfg::requiring_test(&module.attrs).is_some() =>
        {
            (Some(module), rest)
        }
        _ => (None, items),
    }
}

#[derive(Default)]
struct Production {
    report: Report,
    inline: Vec<String>,
    test_functions: TestFunctions,
}

impl Production {
    /// Check the file's closing test module declaration.
    fn trailing(&mut self, path: &Utf8Path, owns_dir: bool, module: &ItemMod) {
        let decl = declaration(module, &self.inline);
        if module.ident != "tests" {
            self.report
                .breaches
                .push((decl.line, Breach::ExtraTestModule {
                    name: decl.name.clone(),
                }));
            self.report
                .children
                .push((decl, Role::Tests(TestForm::Nested)));
            return;
        }

        let form = if layout::is_mod_rs(path) {
            // Reported as a `mod.rs`; where its tests go follows the rename.
            TestForm::Nested
        } else {
            match layout::test_form(path, owns_dir, decl.path.as_deref()) {
                Ok(form) => form,
                Err(expected) => {
                    self.report
                        .breaches
                        .push((decl.line, Breach::TestModulePath {
                            found: decl.path.clone(),
                            expected,
                        }));
                    TestForm::Nested
                }
            }
        };
        self.report.children.push((decl, Role::Tests(form)));
    }

    /// Report `attrs` when they hold a `cfg` requiring `test`, naming the item
    /// as `what`.
    ///
    /// Returns whether it reported, so the caller skips what the item holds.
    fn test_only(&mut self, attrs: &[Attribute], what: &'static str) -> bool {
        let Some(attr) = cfg::requiring_test(attrs) else {
            return false;
        };
        self.report
            .breaches
            .push((attr_line(attr), Breach::TestOnly { what }));
        true
    }

    fn module(&mut self, module: &ItemMod) {
        let test = cfg::requiring_test(&module.attrs).is_some();
        match (&module.content, test) {
            (Some(_), true) => {
                self.report
                    .breaches
                    .push((line(module.ident.span()), Breach::InlineTestModule));
            }
            (Some((_, items)), false) => {
                for attr in &module.attrs {
                    self.visit_attribute(attr);
                }
                self.inline.push(module.ident.to_string());
                for item in items {
                    self.visit_item(item);
                }
                self.inline.pop();
            }
            (None, true) => {
                let decl = declaration(module, &self.inline);
                let breach = if module.ident == "tests" {
                    Breach::DeclarationNotLast
                } else {
                    Breach::ExtraTestModule {
                        name: decl.name.clone(),
                    }
                };
                self.report.breaches.push((decl.line, breach));
                self.report
                    .children
                    .push((decl, Role::Tests(TestForm::Nested)));
            }
            (None, false) => {
                for attr in &module.attrs {
                    self.visit_attribute(attr);
                }
                let decl = declaration(module, &self.inline);
                self.report.children.push((decl, Role::Production));
            }
        }
    }
}

impl<'ast> Visit<'ast> for Production {
    fn visit_item(&mut self, item: &'ast Item) {
        if let Item::Mod(module) = item {
            return self.module(module);
        }
        if self.test_only(item_attrs(item), item_kind(item)) {
            return;
        }
        visit::visit_item(self, item);
    }

    fn visit_impl_item(&mut self, item: &'ast ImplItem) {
        let (attrs, what) = match item {
            ImplItem::Const(item) => (&item.attrs[..], "associated constant"),
            ImplItem::Fn(item) => (&item.attrs[..], "method"),
            ImplItem::Type(item) => (&item.attrs[..], "associated type"),
            ImplItem::Macro(item) => (&item.attrs[..], "macro invocation"),
            _ => (&[][..], "item"),
        };
        if !self.test_only(attrs, what) {
            visit::visit_impl_item(self, item);
        }
    }

    fn visit_trait_item(&mut self, item: &'ast TraitItem) {
        let (attrs, what) = match item {
            TraitItem::Const(item) => (&item.attrs[..], "associated constant"),
            TraitItem::Fn(item) => (&item.attrs[..], "method"),
            TraitItem::Type(item) => (&item.attrs[..], "associated type"),
            TraitItem::Macro(item) => (&item.attrs[..], "macro invocation"),
            _ => (&[][..], "item"),
        };
        if !self.test_only(attrs, what) {
            visit::visit_trait_item(self, item);
        }
    }

    fn visit_foreign_item(&mut self, item: &'ast ForeignItem) {
        let attrs = match item {
            ForeignItem::Fn(item) => &item.attrs[..],
            ForeignItem::Static(item) => &item.attrs[..],
            ForeignItem::Type(item) => &item.attrs[..],
            ForeignItem::Macro(item) => &item.attrs[..],
            _ => &[][..],
        };
        if !self.test_only(attrs, "foreign item") {
            visit::visit_foreign_item(self, item);
        }
    }

    fn visit_field(&mut self, field: &'ast Field) {
        if !self.test_only(&field.attrs, "field") {
            visit::visit_field(self, field);
        }
    }

    fn visit_variant(&mut self, variant: &'ast Variant) {
        if !self.test_only(&variant.attrs, "variant") {
            visit::visit_variant(self, variant);
        }
    }

    fn visit_arm(&mut self, arm: &'ast Arm) {
        if !self.test_only(&arm.attrs, "match arm") {
            visit::visit_arm(self, arm);
        }
    }

    fn visit_local(&mut self, local: &'ast Local) {
        if !self.test_only(&local.attrs, "statement") {
            visit::visit_local(self, local);
        }
    }

    fn visit_field_value(&mut self, field: &'ast FieldValue) {
        if !self.test_only(&field.attrs, "field initializer") {
            visit::visit_field_value(self, field);
        }
    }

    // Every node the methods above do not intercept, such as an expression or a
    // parameter, still has its attributes visited here.
    fn visit_attribute(&mut self, attr: &'ast Attribute) {
        match cfg::classify(attr) {
            TestCfg::Requires => self
                .report
                .breaches
                .push((attr_line(attr), Breach::TestOnly { what: "code" })),
            TestCfg::Excludes => self
                .report
                .breaches
                .push((attr_line(attr), Breach::NotTest)),
            TestCfg::Neither => {}
        }
        if cfg::is_test_function(attr) {
            self.test_functions.note(attr_line(attr));
        }
    }
}

#[derive(Default)]
struct Tests {
    report: Report,
    inline: Vec<String>,
    test_functions: TestFunctions,

    /// The line of the first module the file declares.
    first_declaration: Option<usize>,
}

impl<'ast> Visit<'ast> for Tests {
    fn visit_item_mod(&mut self, module: &'ast ItemMod) {
        if let Some((_, items)) = &module.content {
            self.inline.push(module.ident.to_string());
            for item in items {
                self.visit_item(item);
            }
            self.inline.pop();
            return;
        }

        let decl = declaration(module, &self.inline);
        self.first_declaration.get_or_insert(decl.line);
        match &decl.path {
            None => self
                .report
                .breaches
                .push((decl.line, Breach::UnpathedTestModule {
                    name: decl.name.clone(),
                })),
            Some(path) if path.contains(['/', '\\']) => {
                self.report
                    .breaches
                    .push((decl.line, Breach::NotSibling { path: path.clone() }));
            }
            Some(path) if !layout::may_declare_from_tests(Utf8Path::new(path)) => {
                self.report
                    .breaches
                    .push((decl.line, Breach::SupportModuleName { path: path.clone() }));
            }
            Some(_) => {}
        }
        self.report
            .children
            .push((decl, Role::Tests(TestForm::Nested)));
    }

    fn visit_attribute(&mut self, attr: &'ast Attribute) {
        if cfg::is_test_function(attr) {
            self.test_functions.note(attr_line(attr));
        }
    }
}

/// The `#[test]` functions a file holds: how many, and the line of the first.
#[derive(Default)]
struct TestFunctions {
    first: Option<usize>,
    count: usize,
}

impl TestFunctions {
    fn note(&mut self, line: usize) {
        self.first.get_or_insert(line);
        self.count += 1;
    }

    /// Add one breach for all of them to `report`, if there are any.
    fn report(&self, report: &mut Report) {
        if let Some(line) = self.first {
            report
                .breaches
                .push((line, Breach::TestFunctions { count: self.count }));
        }
    }
}

fn declaration(module: &ItemMod, inline: &[String]) -> Declaration {
    Declaration {
        name: module.ident.to_string(),
        line: line(module.ident.span()),
        path: path_attribute(&module.attrs),
        inline: inline.to_vec(),
    }
}

/// The value of a `#[path = "…"]` attribute.
fn path_attribute(attrs: &[Attribute]) -> Option<String> {
    attrs.iter().find_map(|attr| match &attr.meta {
        Meta::NameValue(meta) if meta.path.is_ident("path") => match &meta.value {
            syn::Expr::Lit(expr) => match &expr.lit {
                Lit::Str(value) => Some(value.value()),
                _ => None,
            },
            _ => None,
        },
        _ => None,
    })
}

fn item_attrs(item: &Item) -> &[Attribute] {
    match item {
        Item::Const(item) => &item.attrs,
        Item::Enum(item) => &item.attrs,
        Item::ExternCrate(item) => &item.attrs,
        Item::Fn(item) => &item.attrs,
        Item::ForeignMod(item) => &item.attrs,
        Item::Impl(item) => &item.attrs,
        Item::Macro(item) => &item.attrs,
        Item::Mod(item) => &item.attrs,
        Item::Static(item) => &item.attrs,
        Item::Struct(item) => &item.attrs,
        Item::Trait(item) => &item.attrs,
        Item::TraitAlias(item) => &item.attrs,
        Item::Type(item) => &item.attrs,
        Item::Union(item) => &item.attrs,
        Item::Use(item) => &item.attrs,
        _ => &[],
    }
}

fn item_kind(item: &Item) -> &'static str {
    match item {
        Item::Const(_) => "constant",
        Item::Enum(_) => "enum",
        Item::ExternCrate(_) => "extern crate",
        Item::Fn(_) => "function",
        Item::ForeignMod(_) => "extern block",
        Item::Impl(_) => "impl block",
        Item::Macro(_) => "macro invocation",
        Item::Mod(_) => "module",
        Item::Static(_) => "static",
        Item::Struct(_) => "struct",
        Item::Trait(_) => "trait",
        Item::TraitAlias(_) => "trait alias",
        Item::Type(_) => "type alias",
        Item::Union(_) => "union",
        Item::Use(_) => "import",
        _ => "item",
    }
}

fn line(span: Span) -> usize {
    span.start().line
}

/// The line an attribute starts on, at its `#`.
fn attr_line(attr: &Attribute) -> usize {
    line(attr.pound_token.spans[0])
}
