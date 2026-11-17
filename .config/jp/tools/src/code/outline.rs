//! Flattened document symbol outline for mapping source lines to enclosing
//! items and resolving qualified symbol names.

use super::{
    protocol::{DocumentSymbol, Position, Range, kind},
    rust_analyzer::{PATH_SEPARATOR, base_ident, doc_text, impl_parts, impl_segment, is_preamble},
};

/// A document symbol with its enclosing scope resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    /// Unqualified symbol name (`iter`, `Stream`, `impl Render for Stream`).
    pub ident: String,

    /// Name qualified by enclosing items in the same file (`Stream::iter`,
    /// `<Stream as Render>::render`, `tests::counts`).
    pub name: String,

    pub kind: u32,

    /// Full span of the item, including doc comments and attributes.
    pub range: Range,

    /// Span of the item's identifier.
    pub selection: Range,

    pub parent: Option<usize>,

    /// Nesting depth (number of enclosing items).
    pub depth: usize,

    /// Enclosing item names from outermost to innermost; `impl` blocks
    /// contribute their `Self` type name.
    pub scope: Vec<String>,

    /// Whether the item is inside a `test` or `tests` module.
    pub in_tests: bool,
}

/// Flatten a hierarchical `DocumentSymbol` tree in pre-order (parents before
/// children).
#[must_use]
pub fn flatten(symbols: &[DocumentSymbol]) -> Vec<Item> {
    let mut items = vec![];
    for symbol in symbols {
        push(&mut items, symbol, None, &Prefix::default());
    }

    items
}

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

    // Qualify children of an `impl` block by its `Self` type rather than the
    // `impl ...` header string.
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

/// Return the innermost item spanning `line`, excluding struct fields and enum
/// variants so references in field or variant types are attributed to the
/// enclosing type definition.
#[must_use]
pub fn enclosing(items: &[Item], line: u32) -> Option<usize> {
    innermost(items, |item| {
        item.range.spans_line(line) && !matches!(item.kind, kind::FIELD | kind::ENUM_MEMBER)
    })
}

/// Return the innermost item whose identifier selection contains `position`,
/// falling back to the innermost item whose full range contains `position`.
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

/// Return whether any item's identifier selection contains `position`.
///
/// Distinguishes item definitions from `pub use` re-exports, which are omitted
/// from document symbols.
#[must_use]
pub fn defines(items: &[Item], position: Position) -> bool {
    items.iter().any(|item| item.selection.contains(position))
}

/// Return the indices of items named `ident` whose enclosing `scope` ends with
/// `qualifier`.
#[must_use]
pub fn matching(items: &[Item], qualifier: &[String], ident: &str) -> Vec<usize> {
    items
        .iter()
        .enumerate()
        .filter(|(_, item)| item.ident == ident && item.scope.ends_with(qualifier))
        .map(|(index, _)| index)
        .collect()
}

/// Return the indices of direct children of `parent` named `ident`.
#[must_use]
pub fn children_named(items: &[Item], parent: usize, ident: &str) -> Vec<usize> {
    items
        .iter()
        .enumerate()
        .filter(|(_, item)| item.parent == Some(parent) && item.ident == ident)
        .map(|(index, _)| index)
        .collect()
}

/// Return the first line of `item` after any leading doc comments and
/// attributes.
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

/// Extract the first paragraph of `item`'s outer doc comment, truncated to at
/// most `max` lines.
///
/// `rust-analyzer` includes leading doc comments in `item.range`, so doc lines
/// appear between `item.range.start.line` and [`code_start`].
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
