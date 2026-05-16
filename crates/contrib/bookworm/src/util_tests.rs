use super::*;

#[test]
fn html_to_markdown_converts_paragraphs() {
    let md = html_to_markdown("<p>Hello world.</p>").unwrap();
    assert_eq!(md, "Hello world.");
}

#[test]
fn html_to_markdown_converts_rustdoc_docblock_shape() {
    let html = r"<p>Looks up a value by a JSON Pointer.</p>
        <h5 id='examples'>Examples</h5>
        <pre><code>let data = json!({});</code></pre>";
    let md = html_to_markdown(html).unwrap();

    assert!(md.contains("Looks up a value"), "got:\n{md}");
    assert!(md.contains("##### Examples"), "got:\n{md}");
    assert!(md.contains("let data = json!"), "got:\n{md}");
}

#[test]
fn html_to_markdown_drops_script_and_style() {
    let html = "<p>real content</p><script>alert(1)</script><style>p { color: red }</style>";
    let md = html_to_markdown(html).unwrap();

    assert!(md.contains("real content"));
    assert!(!md.contains("alert"));
    assert!(!md.contains("color: red"));
}

#[test]
fn html_to_markdown_caps_blank_lines() {
    let html = "<p>one</p><p>two</p><p>three</p>";
    let md = html_to_markdown(html).unwrap();

    // Three paragraphs separated by exactly one blank line each (so max 2
    // consecutive newlines between them).
    assert!(!md.contains("\n\n\n"), "got triple newlines:\n{md}");
}

#[test]
fn html_to_markdown_trims_trailing_whitespace() {
    let md = html_to_markdown("<p>content</p>\n\n\n").unwrap();
    assert!(!md.ends_with('\n'));
    assert!(!md.ends_with(' '));
}
