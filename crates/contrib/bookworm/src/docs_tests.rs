use scraper::Html;

use super::*;

/// Realistic rustdoc item page (synthesised — covers the structural quirks
/// without docs.rs scaffolding).
///
/// Notable features:
/// - `<pre class="rust item-decl">` carries the top-level signature.
/// - The type's prose lives inside `<details class="toggle top-doc">`, which
///   wraps a `<div class="docblock">`. Older rustdoc emits the docblock as a
///   direct child of `<main>`; we test that fallback below.
/// - Variants and impls use `<section id="...">` wrappers; the docblock is
///   the next sibling, OR the next sibling of `<summary>` if the section is
///   inside a `<details>` toggle.
const RUSTDOC_PAGE: &str = r##"<!DOCTYPE html>
<html>
<head><title>Value in serde_json::value - Rust</title></head>
<body>
<main>
  <section id="main-content">
    <div class="main-heading">
      <h1>Enum <a>serde_json::value::</a><span>Value</span></h1>
      <span class="out-of-band"><a class="src" href="../../src/serde_json/value/mod.rs.html#116-176">Source</a></span>
    </div>
    <pre class="rust item-decl"><code>pub enum <a class="enum">Value</a> {
    <a class="variant" href="#variant.Null">Null</a>,
    <a class="variant" href="#variant.Bool">Bool</a>(<a class="primitive">bool</a>),
    <a class="variant" href="#variant.Number">Number</a>(<a class="struct">Number</a>),
}</code></pre>
    <details class="toggle top-doc" open>
      <summary class="hideme"><span>Expand description</span></summary>
      <div class="docblock"><p>Represents any valid JSON value.</p></div>
    </details>
    <h2 id="variants" class="section-header">Variants</h2>
    <ul>
      <li>
        <section id="variant.Null" class="variant"><h3 class="code-header">Null</h3></section>
        <div class="docblock"><p>Represents a JSON null value.</p></div>
      </li>
      <li>
        <section id="variant.Bool" class="variant"><h3 class="code-header">Bool(<a>bool</a>)</h3></section>
        <div class="docblock"><p>Represents a JSON boolean.</p></div>
      </li>
    </ul>
    <h2 id="implementations">Implementations</h2>
    <details class="toggle implementors-toggle">
      <summary><section id="impl-Default-for-Value">
        <a class="src" href="../../src/serde_json/value/mod.rs.html#900">Source</a>
        <h3 class="code-header">impl <a>Default</a> for Value</h3>
      </section></summary>
      <div class="docblock"><p>The default value is Value::Null.</p></div>
      <div class="impl-items">
        <details class="toggle method-toggle">
          <summary><section id="method.pointer">
            <a class="src" href="../../src/serde_json/value/mod.rs.html#400">Source</a>
            <h4 class="code-header">pub fn <a class="fn">pointer</a>&lt;'a&gt;(&amp;'a self, pointer: &amp;<a class="primitive">str</a>) -&gt; <a class="enum">Option</a>&lt;&amp;'a <a class="enum">Value</a>&gt;</h4>
          </section></summary>
          <div class="docblock">
            <p>Looks up a value by a JSON Pointer.</p>
            <h5 id="examples">Examples</h5>
            <pre>let data = json!({});</pre>
          </div>
        </details>
      </div>
    </details>
  </section>
</main>
</body>
</html>"##;

/// Older rustdoc shape: top-doc is a direct `<div class="docblock">` child of
/// `<main>`, no `<details class="toggle top-doc">` wrapper. We keep this
/// fallback so older downloaded docsets still extract.
const RUSTDOC_PAGE_NO_TOGGLE: &str = r#"<!DOCTYPE html>
<html>
<body>
<section id="main-content">
  <h1>Struct Foo</h1>
  <pre class="rust item-decl">pub struct Foo;</pre>
  <div class="docblock"><p>A simple unit struct.</p></div>
  <h2>Implementations</h2>
</section>
</body>
</html>"#;

fn parse(html: &str) -> Html {
    Html::parse_document(html)
}

fn select_main(doc: &Html) -> ElementRef<'_> {
    doc.select(&MAIN_CONTENT)
        .next()
        .expect("fixture has #main-content")
}

fn select_id<'a>(doc: &'a Html, id: &str) -> ElementRef<'a> {
    let selector_str = format!(r#"[id="{}"]"#, escape_css_value(id));
    let selector = Selector::parse(&selector_str).expect("valid id selector");
    // Leak the selector for the test's lifetime — fine for unit tests.
    let leaked: &'static Selector = Box::leak(Box::new(selector));
    doc.select(leaked)
        .next()
        .unwrap_or_else(|| panic!("no element with id `{id}`"))
}

// --- primary (no-fragment) ---

#[test]
fn primary_signature_returns_item_decl_text() {
    let doc = parse(RUSTDOC_PAGE);
    let sig = extract_primary_signature(select_main(&doc)).expect("signature");

    assert!(sig.starts_with("pub enum Value"), "got: {sig:?}");
    assert!(sig.contains("Null,"), "got: {sig:?}");
    assert!(sig.contains("Bool(bool)"), "got: {sig:?}");
    // Plain text — no anchor markup.
    assert!(
        !sig.contains("<a"),
        "signature should be plain text: {sig:?}"
    );
}

#[test]
fn primary_documentation_uses_top_doc_toggle() {
    let doc = parse(RUSTDOC_PAGE);
    let docs = extract_primary_documentation(select_main(&doc)).expect("documentation");

    assert!(
        docs.contains("Represents any valid JSON value"),
        "got: {docs:?}"
    );
    // Must NOT include variant docs, impl docs, or method docs.
    assert!(
        !docs.contains("Represents a JSON null value"),
        "variant doc leaked: {docs:?}"
    );
    assert!(
        !docs.contains("The default value is Value::Null"),
        "impl doc leaked: {docs:?}"
    );
    assert!(
        !docs.contains("Looks up a value"),
        "method doc leaked: {docs:?}"
    );
}

#[test]
fn primary_documentation_falls_back_to_direct_child_docblock() {
    let doc = parse(RUSTDOC_PAGE_NO_TOGGLE);
    let docs = extract_primary_documentation(select_main(&doc)).expect("documentation");

    assert!(docs.contains("A simple unit struct"), "got: {docs:?}");
}

#[test]
fn primary_documentation_is_none_when_no_top_doc() {
    // Page with only nested item docblocks; no top-doc and no direct child.
    let html = r#"<html><body><section id="main-content">
        <h1>Enum Foo</h1>
        <h2>Variants</h2>
        <ul><li>
          <section id="variant.A"><h3>A</h3></section>
          <div class="docblock"><p>The A variant.</p></div>
        </li></ul>
      </section></body></html>"#;
    let doc = parse(html);
    let docs = extract_primary_documentation(select_main(&doc));

    assert_eq!(docs, None, "no top-doc means no documentation");
}

// --- fragment (rustdoc section) ---

#[test]
fn fragment_signature_strips_anchor_markup() {
    let doc = parse(RUSTDOC_PAGE);
    let sig = extract_fragment_signature(select_id(&doc, "method.pointer")).expect("signature");

    assert_eq!(
        sig,
        "pub fn pointer<'a>(&'a self, pointer: &str) -> Option<&'a Value>"
    );
}

#[test]
fn fragment_signature_for_variant_is_just_the_name() {
    let doc = parse(RUSTDOC_PAGE);
    let sig = extract_fragment_signature(select_id(&doc, "variant.Null")).expect("signature");
    assert_eq!(sig, "Null");
}

#[test]
fn fragment_signature_for_impl() {
    let doc = parse(RUSTDOC_PAGE);
    let sig =
        extract_fragment_signature(select_id(&doc, "impl-Default-for-Value")).expect("signature");
    assert_eq!(sig, "impl Default for Value");
}

#[test]
fn fragment_documentation_for_method_in_toggle() {
    let doc = parse(RUSTDOC_PAGE);
    let docs =
        extract_fragment_documentation(select_id(&doc, "method.pointer")).expect("documentation");

    assert!(docs.contains("Looks up a value"), "got: {docs:?}");
    assert!(
        docs.contains("Examples"),
        "preserves docblock children: {docs:?}"
    );
}

#[test]
fn fragment_documentation_for_variant_flat_sibling() {
    let doc = parse(RUSTDOC_PAGE);
    let docs =
        extract_fragment_documentation(select_id(&doc, "variant.Null")).expect("documentation");

    assert!(
        docs.contains("Represents a JSON null value"),
        "got: {docs:?}"
    );
}

#[test]
fn fragment_documentation_for_impl_does_not_include_methods() {
    let doc = parse(RUSTDOC_PAGE);
    let docs = extract_fragment_documentation(select_id(&doc, "impl-Default-for-Value"))
        .expect("documentation");

    assert!(
        docs.contains("The default value is Value::Null"),
        "got: {docs:?}"
    );
    // Crucially: must stop at the first element sibling. The `<div class="impl-items">`
    // contains every method's docblock, which we must NOT walk into.
    assert!(
        !docs.contains("Looks up a value"),
        "method doc leaked into impl: {docs:?}"
    );
}

#[test]
fn fragment_documentation_returns_none_when_no_docblock() {
    let html = r#"<html><body>
        <section id="solo"><h3>Solo</h3></section>
        <p>An unrelated paragraph.</p>
      </body></html>"#;
    let doc = parse(html);
    let docs = extract_fragment_documentation(select_id(&doc, "solo"));

    // First element sibling is <p>, not a docblock — stop without returning it.
    assert_eq!(docs, None);
}

// --- is_docblock requires <div> ---

#[test]
fn is_docblock_rejects_non_div_with_docblock_class() {
    let html = r#"<html><body><section><span class="docblock">not a docblock</span></section></body></html>"#;
    let doc = parse(html);
    let span = doc
        .select(&Selector::parse("span").unwrap())
        .next()
        .unwrap();
    assert!(!is_docblock(span));
}

#[test]
fn is_docblock_accepts_div_with_multiclass() {
    let html = r#"<html><body><div class="docblock item-decl"></div></body></html>"#;
    let doc = parse(html);
    let div = doc.select(&Selector::parse("div").unwrap()).next().unwrap();
    assert!(is_docblock(div));
}

// --- escape_css_value ---

#[test]
fn escape_css_value_escapes_quotes_and_backslashes() {
    assert_eq!(escape_css_value(r#"a"b\c"#), r#"a\"b\\c"#);
    assert_eq!(escape_css_value("method.foo"), "method.foo");
    assert_eq!(escape_css_value(""), "");
}

// --- strip_doc_anchors ---

#[test]
fn strip_doc_anchors_removes_rustdoc_permalinks() {
    let html = r##"<h5 id="examples"><a class="doc-anchor" href="#examples">§</a>Examples</h5>
<p>Some prose.</p>"##;
    let stripped = strip_doc_anchors(html);
    assert_eq!(
        stripped,
        "<h5 id=\"examples\">Examples</h5>\n<p>Some prose.</p>"
    );
}

#[test]
fn strip_doc_anchors_leaves_other_anchors_alone() {
    // Only `class="doc-anchor"` anchors are stripped; ordinary links stay.
    let html = r#"<p>See <a href="https://example.com">example</a>.</p>"#;
    assert_eq!(strip_doc_anchors(html), html);
}

#[test]
fn strip_doc_anchors_is_noop_when_absent() {
    let html = "<p>Plain prose, no anchors.</p>";
    assert_eq!(strip_doc_anchors(html), html);
}
