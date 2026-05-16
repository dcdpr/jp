use std::{
    fs,
    path::{Path, PathBuf},
    sync::LazyLock,
};

use rusqlite::Connection;
use scraper::{ElementRef, Html, Selector};
use serde::Serialize;

use crate::{error::Error, util::html_to_markdown};

static MAIN_CONTENT: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse("#main-content").expect("static selector"));
static ITEM_DECL: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse("pre.rust.item-decl").expect("static selector"));
static TOP_DOC_DOCBLOCK: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse("details.top-doc div.docblock").expect("static selector"));
static SECTION_HEADING: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse("h1, h2, h3, h4, h5, h6").expect("static selector"));
static A_SRC: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse("a.src").expect("static selector"));
static DOC_ANCHOR: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse("a.doc-anchor").expect("static selector"));

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Item {
    pub path: String,
    pub kind: String,
    /// Type declaration as plain text (anchors flattened).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub type_info: Option<String>,
    /// Item documentation in Markdown, converted from rustdoc's HTML.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub documentation: Option<String>,
    /// Source file path relative to the docset root. Used internally to
    /// compute `src_resource` URIs and to deduplicate re-exported items.
    /// Not serialised — callers expose the URI form instead.
    #[serde(skip)]
    pub src_path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SrcMatch {
    pub path: String,
    pub line: usize,
    pub column: usize,
    pub context: String,
}

pub struct Docs<'a> {
    root: PathBuf,
    conn: &'a Connection,
}

impl<'a> Docs<'a> {
    /// Create a new `Docs` instance.
    pub fn new(root: impl Into<PathBuf>, conn: &'a Connection) -> Result<Self, Error> {
        let root = root.into();
        if !root.is_dir() {
            return Err(Error::MissingDocs);
        }

        rusqlite::vtab::array::load_module(conn)?;

        Ok(Self { root, conn })
    }

    /// Get the item details for a given item path.
    pub fn item(&self, path: &str) -> Result<Item, Error> {
        // Look up the index row by the FULL path (including any `#fragment`)
        // — the indexer stores method/variant/impl entries at
        // `enum.Value.html#method.pointer` etc., so stripping the fragment
        // before the query would always resolve to the enclosing item (the
        // enum) instead of the actual matched entry (the method).
        let (name, kind) = self.conn.query_row(
            "SELECT name, type FROM searchIndex WHERE path = ?",
            [path],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )?;

        // The HTML file lives at the path *without* the fragment; the
        // fragment is used to pick an element inside the file.
        let (path, fragment) = path.rsplit_once('#').unwrap_or((path, ""));

        let html = fs::read_to_string(self.root.join(path))?;
        let document = Html::parse_document(&html);

        let (type_info, doc_html, src_path) = if fragment.is_empty() {
            let main = document
                .select(&MAIN_CONTENT)
                .next()
                .ok_or(Error::NotFound)?;
            (
                extract_primary_signature(main),
                extract_primary_documentation(main),
                extract_src_path(main, &self.root, path),
            )
        } else {
            let selector_str = format!(r#"[id="{}"]"#, escape_css_value(fragment));
            let selector = Selector::parse(&selector_str).map_err(|e| {
                Error::HtmlParsing(format!("invalid id selector {selector_str}: {e}"))
            })?;
            let target = document.select(&selector).next().ok_or(Error::NotFound)?;
            (
                extract_fragment_signature(target),
                extract_fragment_documentation(target),
                extract_src_path(target, &self.root, path),
            )
        };

        let documentation = doc_html
            .map(|html| strip_doc_anchors(&html))
            .map(|html| html_to_markdown(&html))
            .transpose()?
            .filter(|s| !s.is_empty());

        Ok(Item {
            path: name,
            kind,
            type_info,
            documentation,
            src_path,
        })
    }

    pub fn search_src(&self, _query: &str) -> Result<Vec<SrcMatch>, Error> {
        Ok(vec![])
    }
}

/// Pull the top-level type signature out of `<pre class="rust item-decl">`.
/// Returns the plain-text content (anchors flattened) trimmed.
fn extract_primary_signature(main: ElementRef<'_>) -> Option<String> {
    let pre = main.select(&ITEM_DECL).next()?;
    let text = pre.text().collect::<String>();
    let trimmed = text.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_owned())
    }
}

/// Pull the top-doc out of `<main>`.
///
/// Modern rustdoc wraps it in `<details class="toggle top-doc">`; older
/// docsets emit a `<div class="docblock">` as a direct child of `<main>`.
/// Returns `None` if the type has no top-level prose (we do **not** fall
/// through to nested item docblocks — those belong to variants/methods).
fn extract_primary_documentation(main: ElementRef<'_>) -> Option<String> {
    if let Some(el) = main.select(&TOP_DOC_DOCBLOCK).next() {
        return Some(el.inner_html());
    }

    main.children()
        .filter_map(ElementRef::wrap)
        .find(|el| is_docblock(*el))
        .map(|el| el.inner_html())
}

/// Extract the signature for a rustdoc `<section id="...">` (or any element
/// targeted by a fragment). Looks for the first heading inside the section
/// and returns its plain-text content (anchors flattened).
fn extract_fragment_signature(section: ElementRef<'_>) -> Option<String> {
    let heading = section.select(&SECTION_HEADING).next()?;
    let text = heading.text().collect::<String>();
    let trimmed = text.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_owned())
    }
}

/// Extract the docblock paired with a rustdoc section.
///
/// The docblock is the immediately-following element sibling of the section.
/// When the section is wrapped in `<summary>` (rustdoc's `<details>` toggle
/// layout), the docblock is a sibling of the `<summary>`, not of the section
/// itself. We stop at the first element sibling unconditionally — if it isn't
/// a docblock, there are no docs. Walking further would risk picking up the
/// `<div class="impl-items">` (which contains every nested method's docblock).
fn extract_fragment_documentation(section: ElementRef<'_>) -> Option<String> {
    let anchor = section
        .parent()
        .and_then(ElementRef::wrap)
        .filter(|el| el.value().name() == "summary")
        .unwrap_or(section);

    for sib in anchor.next_siblings() {
        let Some(el) = ElementRef::wrap(sib) else {
            continue;
        };
        if is_docblock(el) {
            return Some(el.inner_html());
        }
        // First element sibling decides: docblock or unrelated, stop either way.
        break;
    }

    None
}

/// Resolve `<a class="src">` to a canonical path relative to `root`.
fn extract_src_path(element: ElementRef<'_>, root: &Path, item_path: &str) -> Option<String> {
    let href = element.select(&A_SRC).next()?.value().attr("href")?;
    let (src, fragment) = href.split_once('#').unwrap_or((href, ""));

    let absolute = root
        .join(Path::new(item_path).parent().unwrap_or(Path::new("")))
        .join(src)
        .canonicalize()
        .ok()?;
    let abs_str = format!("{}#{fragment}", absolute.to_string_lossy());
    let root_str = root.canonicalize().ok()?.to_string_lossy().into_owned();

    abs_str.strip_prefix(&root_str).map(ToOwned::to_owned)
}

/// Remove rustdoc's `<a class="doc-anchor">§</a>` permalink anchors from a
/// docblock HTML fragment. These render as ugly `[§](#anchor)` Markdown
/// links inside section headings (`##### [§](#examples)Examples`) and carry
/// no value once the docblock has been extracted from its surrounding page.
fn strip_doc_anchors(html: &str) -> String {
    let fragment = Html::parse_fragment(html);
    let mut out = html.to_owned();
    for el in fragment.select(&DOC_ANCHOR) {
        let outer = el.html();
        out = out.replace(&outer, "");
    }
    out
}

/// True if `el` is a `<div class="docblock">`. Class attribute may have
/// multiple values; we check word-membership.
fn is_docblock(el: ElementRef<'_>) -> bool {
    el.value().name() == "div"
        && el
            .value()
            .attr("class")
            .is_some_and(|c| c.split_ascii_whitespace().any(|cl| cl == "docblock"))
}

/// Escape characters with special meaning inside a CSS attribute value
/// selector (`[id="..."]`). Only `"` and `\` need escaping for our use.
fn escape_css_value(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        if matches!(ch, '"' | '\\') {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

#[cfg(test)]
#[path = "docs_tests.rs"]
mod tests;
