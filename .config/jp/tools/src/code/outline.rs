//! A file's items, flattened from the server's document symbols: which item
//! encloses a line, and which items a qualified name can mean.

use super::{
    protocol::{DocumentSymbol, Position, Range, kind},
    rust_analyzer::{PATH_SEPARATOR, base_ident, doc_text, impl_parts, impl_segment, is_preamble},
};

/// One item in a file, with the items that enclose it already resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    /// The symbol's own name: `iter`, `Stream`, `impl Render for Stream`.
    pub ident: String,

    /// The name qualified by the enclosing items in the same file:
    /// `Stream::iter`, `<Stream as Render>::render`, `tests::counts`.
    pub name: String,

    pub kind: u32,

    /// The whole item, doc comments and attributes included.
    pub range: Range,

    /// The item's name.
    pub selection: Range,

    pub parent: Option<usize>,

    /// How many items enclose this one.
    pub depth: usize,

    /// The names a qualifier can use for the enclosing items, outermost first.
    /// An `impl` block contributes its self type.
    pub scope: Vec<String>,

    /// Whether a module named `tests` or `test` encloses the item.
    pub in_tests: bool,
}

/// The items of a document symbol tree, parents before their children.
#[must_use]
pub fn flatten(symbols: &[DocumentSymbol]) -> Vec<Item> {
    let mut items = vec![];
    for symbol in symbols {
        push(&mut items, symbol, None, &Prefix::default());
    }

    items
}

/// What an item passes down to its children.
#[derive(Debug, Default)]
struct Prefix {
    name: Option<String>,
    scope: Vec<String>,
    in_tests: bool,
    depth: usize,
}

fn push(items: &mut Vec<Item>, symbol: &DocumentSymbol, parent: Option<usize>, prefix: &Prefix) {
    let name = match (&prefix.name, symbol.kind) {
        (_, kind::OBJECT) | (None, _) => symbol.name.clone(),
        (Some(outer), _) => format!("{outer}{PATH_SEPARATOR}{}", symbol.name),
    };

    // Children of an `impl` are named through its self type, the way they are
    // written at a call site.
    let (segment, scope_name) = match impl_parts(&symbol.name) {
        Some((Some(trait_), self_ty)) if symbol.kind == kind::OBJECT => (
            impl_segment(trait_, self_ty),
            base_ident(self_ty).to_owned(),
        ),
        Some((None, self_ty)) if symbol.kind == kind::OBJECT => {
            (self_ty.to_owned(), base_ident(self_ty).to_owned())
        }
        _ => (symbol.name.clone(), symbol.name.clone()),
    };

    let index = items.len();
    items.push(Item {
        ident: symbol.name.clone(),
        name,
        kind: symbol.kind,
        range: symbol.range,
        selection: symbol.selection_range,
        parent,
        depth: prefix.depth,
        scope: prefix.scope.clone(),
        in_tests: prefix.in_tests,
    });

    let mut scope = prefix.scope.clone();
    scope.push(scope_name);
    let children = Prefix {
        name: Some(match &prefix.name {
            Some(outer) => format!("{outer}{PATH_SEPARATOR}{segment}"),
            None => segment,
        }),
        scope,
        in_tests: prefix.in_tests
            || (symbol.kind == kind::MODULE && super::rust_analyzer::is_test_module(&symbol.name)),
        depth: prefix.depth + 1,
    };

    for child in &symbol.children {
        push(items, child, Some(index), &children);
    }
}

/// The innermost item that encloses `line`, other than a field or a variant.
///
/// A reference in a field's type is reported against the struct.
#[must_use]
pub fn enclosing(items: &[Item], line: u32) -> Option<usize> {
    innermost(items, |item| {
        item.range.spans_line(line) && !matches!(item.kind, kind::FIELD | kind::ENUM_MEMBER)
    })
}

/// The innermost item whose name is at `position`, or failing that, whose range
/// holds it.
#[must_use]
pub fn at(items: &[Item], position: Position) -> Option<usize> {
    innermost(items, |item| item.selection.contains(position))
        .or_else(|| innermost(items, |item| item.range.contains(position)))
}

fn innermost(items: &[Item], matches: impl Fn(&Item) -> bool) -> Option<usize> {
    items
        .iter()
        .enumerate()
        .filter(|(_, item)| matches(item))
        .max_by_key(|(_, item)| item.depth)
        .map(|(index, _)| index)
}

/// Whether an item's name starts at `position`, which tells a definition from a
/// `use` that re-exports it: imports are not document symbols.
#[must_use]
pub fn defines(items: &[Item], position: Position) -> bool {
    items.iter().any(|item| item.selection.contains(position))
}

/// The items named `ident` whose enclosing names end with `qualifier`.
#[must_use]
pub fn matching(items: &[Item], qualifier: &[String], ident: &str) -> Vec<usize> {
    items
        .iter()
        .enumerate()
        .filter(|(_, item)| item.ident == ident && item.scope.ends_with(qualifier))
        .map(|(index, _)| index)
        .collect()
}

/// The direct children of item `parent` named `ident`.
#[must_use]
pub fn children_named(items: &[Item], parent: usize, ident: &str) -> Vec<usize> {
    items
        .iter()
        .enumerate()
        .filter(|(_, item)| item.parent == Some(parent) && item.ident == ident)
        .map(|(index, _)| index)
        .collect()
}

/// The first line of an item's code, after its doc comments and attributes.
#[must_use]
pub fn code_start(item: &Item, lines: &[String]) -> u32 {
    (item.range.start.line..=item.range.end.line)
        .find(|&line| {
            let text = usize::try_from(line)
                .ok()
                .and_then(|line| lines.get(line))
                .map_or("", |text| text.trim_start());

            !is_preamble(text)
        })
        .unwrap_or(item.range.start.line)
}

/// The first paragraph of an item's doc comment, at most `max` lines.
///
/// rust-analyzer's item range starts at the doc comment, so the comment is the
/// doc lines before [`code_start`].
#[must_use]
pub fn doc_paragraph(item: &Item, lines: &[String], max: usize) -> Vec<String> {
    let start = usize::try_from(item.range.start.line).unwrap_or(usize::MAX);
    let end = usize::try_from(code_start(item, lines)).unwrap_or(usize::MAX);

    let paragraph = lines
        .get(start..end)
        .unwrap_or_default()
        .iter()
        .filter_map(|line| doc_text(line.trim_start()))
        .skip_while(|text| text.is_empty())
        .take_while(|text| !text.is_empty())
        .map(str::to_owned)
        .collect::<Vec<_>>();

    super::render::cap(paragraph, max)
}

#[cfg(test)]
#[path = "outline_tests.rs"]
mod tests;
