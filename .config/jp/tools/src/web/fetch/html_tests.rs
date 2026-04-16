use super::*;

mod collapse_blank_lines {
    use super::*;

    #[test]
    fn preserves_single_newlines() {
        assert_eq!(collapse_blank_lines("a\nb\nc"), "a\nb\nc");
    }

    #[test]
    fn preserves_double_newlines() {
        assert_eq!(collapse_blank_lines("a\n\nb"), "a\n\nb");
    }

    #[test]
    fn collapses_triple_to_double() {
        assert_eq!(collapse_blank_lines("a\n\n\nb"), "a\n\nb");
    }

    #[test]
    fn collapses_many_to_double() {
        assert_eq!(collapse_blank_lines("a\n\n\n\n\n\nb"), "a\n\nb");
    }

    #[test]
    fn trims_trailing_whitespace() {
        assert_eq!(collapse_blank_lines("hello\n\n\n"), "hello");
    }

    #[test]
    fn empty_input() {
        assert_eq!(collapse_blank_lines(""), "");
    }
}

mod html_to_markdown {
    use super::*;

    #[test]
    fn strips_scripts_and_styles() {
        let html = r#"
            <html><body>
                <style>body { color: red; }</style>
                <script>alert('xss')</script>
                <p>Keep this</p>
                <script src="foo.js"></script>
            </body></html>
        "#;
        let md = html_to_markdown(html).unwrap();
        assert!(!md.contains("color: red"));
        assert!(!md.contains("alert"));
        assert!(!md.contains("foo.js"));
        assert!(md.contains("Keep this"));
    }

    #[test]
    fn converts_headings() {
        let html = "<h1>Title</h1><h2>Subtitle</h2><p>Body text</p>";
        let md = html_to_markdown(html).unwrap();
        assert!(md.contains("# Title"));
        assert!(md.contains("## Subtitle"));
        assert!(md.contains("Body text"));
    }

    #[test]
    fn converts_links() {
        let html = r#"<a href="https://example.com">Example</a>"#;
        let md = html_to_markdown(html).unwrap();
        assert!(md.contains("[Example](https://example.com)"));
    }

    #[test]
    fn strips_svg() {
        let html = r#"<p>Before</p><svg><rect width="100" height="100"/></svg><p>After</p>"#;
        let md = html_to_markdown(html).unwrap();
        assert!(!md.contains("rect"));
        assert!(md.contains("Before"));
        assert!(md.contains("After"));
    }

    #[test]
    fn strips_iframes() {
        let html = r#"<p>Text</p><iframe src="https://ads.example.com"></iframe>"#;
        let md = html_to_markdown(html).unwrap();
        assert!(!md.contains("ads.example"));
        assert!(md.contains("Text"));
    }

    #[test]
    fn converts_tables() {
        let html = r"
            <table>
                <thead><tr><th>Name</th><th>Value</th></tr></thead>
                <tbody><tr><td>A</td><td>1</td></tr></tbody>
            </table>
        ";
        let md = html_to_markdown(html).unwrap();
        assert!(md.contains("Name"));
        assert!(md.contains("Value"));
        assert!(md.contains('|'));
    }

    #[test]
    fn empty_body() {
        let html = "<html><head></head><body></body></html>";
        let md = html_to_markdown(html).unwrap();
        assert!(md.trim().is_empty(), "expected empty but got: {md:?}");
    }
}

mod extract_anchor_html {
    use super::*;

    #[test]
    fn heading_with_following_content() {
        let html = r#"
            <html>
            <head><title>My Page</title></head>
            <body>
                <h1>Introduction</h1>
                <p>Intro text</p>
                <h2 id="setup">Setup</h2>
                <p>Setup instructions</p>
                <pre><code>example code</code></pre>
                <h2 id="usage">Usage</h2>
                <p>Usage text</p>
            </body>
            </html>
        "#;

        let result = extract_anchor_html(html, "setup").unwrap();

        assert!(result.contains("<h2"), "should contain the h2 heading");
        assert!(result.contains("Setup"), "should contain heading text");
        assert!(
            result.contains("Setup instructions"),
            "should contain section content"
        );
        assert!(result.contains("example code"), "should contain code block");

        assert!(!result.contains("Introduction"), "should not contain h1");
        assert!(!result.contains("Intro text"), "should not contain intro");
        assert!(
            !result.contains("Usage text"),
            "should not contain next section"
        );

        assert!(
            result.contains("<title>My Page</title>"),
            "should preserve head"
        );
    }

    #[test]
    fn heading_with_nested_subheadings() {
        let html = r#"
            <html>
            <head><title>Docs</title></head>
            <body>
                <h2 id="auth">Authentication</h2>
                <p>Auth overview</p>
                <h3>OAuth</h3>
                <p>OAuth details</p>
                <h3>API Keys</h3>
                <p>Key details</p>
                <h2 id="errors">Errors</h2>
                <p>Error handling</p>
            </body>
            </html>
        "#;

        let result = extract_anchor_html(html, "auth").unwrap();

        assert!(result.contains("Authentication"));
        assert!(result.contains("Auth overview"));
        assert!(result.contains("OAuth"));
        assert!(result.contains("OAuth details"));
        assert!(result.contains("API Keys"));
        assert!(result.contains("Key details"));

        assert!(!result.contains("Error handling"));
    }

    #[test]
    fn container_element() {
        let html = r#"
            <html>
            <head><title>Page</title></head>
            <body>
                <p>Before</p>
                <section id="main-content">
                    <h2>Title</h2>
                    <p>Content here</p>
                </section>
                <p>After</p>
            </body>
            </html>
        "#;

        let result = extract_anchor_html(html, "main-content").unwrap();

        assert!(result.contains("Title"));
        assert!(result.contains("Content here"));
        assert!(!result.contains("Before"));
        assert!(!result.contains("After"));
    }

    #[test]
    fn anchor_not_found() {
        let html = "<html><head></head><body><p>Hello</p></body></html>";
        assert!(extract_anchor_html(html, "nonexistent").is_none());
    }

    #[test]
    fn last_heading_on_page() {
        let html = r#"
            <html>
            <head></head>
            <body>
                <h1>Title</h1>
                <p>Intro</p>
                <h2 id="last">Last Section</h2>
                <p>Final content</p>
                <ul><li>Item 1</li><li>Item 2</li></ul>
            </body>
            </html>
        "#;

        let result = extract_anchor_html(html, "last").unwrap();

        assert!(result.contains("Last Section"));
        assert!(result.contains("Final content"));
        assert!(result.contains("Item 1"));
        assert!(!result.contains("Intro"));
    }

    #[test]
    fn anchor_id_nested_inside_heading() {
        let html = r#"
            <html>
            <head><title>Docs</title></head>
            <body>
                <h2>Previous Section</h2>
                <p>Previous content</p>
                <h3><div id="json-schema-limitations"><div>JSON Schema limitations</div></div></h3>
                <p>Schema limitation details</p>
                <div>More info here</div>
                <h3><div id="next-section"><div>Next Section</div></div></h3>
                <p>Next content</p>
            </body>
            </html>
        "#;

        let result = extract_anchor_html(html, "json-schema-limitations").unwrap();

        assert!(result.contains("JSON Schema limitations"));
        assert!(result.contains("Schema limitation details"));
        assert!(result.contains("More info here"));

        assert!(!result.contains("Previous content"));
        assert!(!result.contains("Next content"));
    }

    #[test]
    fn anchor_with_special_css_characters() {
        let html = r#"
            <html><head></head><body>
                <div id="foo&quot;bar">Content</div>
            </body></html>
        "#;

        drop(extract_anchor_html(html, "foo\"bar"));
    }
}

mod heading_level {
    use super::*;

    #[test]
    fn all_levels() {
        assert_eq!(heading_level("h1"), Some(1));
        assert_eq!(heading_level("h2"), Some(2));
        assert_eq!(heading_level("h3"), Some(3));
        assert_eq!(heading_level("h4"), Some(4));
        assert_eq!(heading_level("h5"), Some(5));
        assert_eq!(heading_level("h6"), Some(6));
    }

    #[test]
    fn non_headings() {
        assert_eq!(heading_level("p"), None);
        assert_eq!(heading_level("div"), None);
        assert_eq!(heading_level("h7"), None);
    }
}

mod escape_css_value {
    use super::*;

    #[test]
    fn no_special_chars() {
        assert_eq!(escape_css_value("hello"), "hello");
    }

    #[test]
    fn escapes_quotes() {
        assert_eq!(escape_css_value(r#"foo"bar"#), r#"foo\"bar"#);
    }

    #[test]
    fn escapes_backslash() {
        assert_eq!(escape_css_value(r"foo\bar"), r"foo\\bar");
    }
}

mod resolve_heading_id {
    use scraper::Html;

    use super::*;

    fn first_heading_id(html: &str) -> Option<String> {
        let doc = Html::parse_document(html);
        let sel = Selector::parse("h1, h2, h3, h4, h5, h6").unwrap();
        let heading = doc.select(&sel).next()?;
        resolve_heading_id(&heading)
    }

    #[test]
    fn pattern1_id_on_heading() {
        let html = r#"<h2 id="setup">Setup</h2>"#;
        assert_eq!(first_heading_id(html).as_deref(), Some("setup"));
    }

    #[test]
    fn pattern2_id_on_parent_section() {
        let html = r#"<section id="ns-containers"><h3>Namespaces</h3></section>"#;
        assert_eq!(first_heading_id(html).as_deref(), Some("ns-containers"));
    }

    #[test]
    fn pattern3_permalink_child_anchor() {
        let html = r##"<h3>Setup <a href="#setup" class="headerlink">¶</a></h3>"##;
        assert_eq!(first_heading_id(html).as_deref(), Some("setup"));
    }

    #[test]
    fn pattern4_child_element_with_id() {
        let html = r#"<h3><div id="json-limits">JSON limits</div></h3>"#;
        assert_eq!(first_heading_id(html).as_deref(), Some("json-limits"));
    }

    #[test]
    fn pattern5_preceding_sibling_anchor() {
        let html = r#"<a id="old-anchor"></a><h2>Old Section</h2>"#;
        assert_eq!(first_heading_id(html).as_deref(), Some("old-anchor"));
    }

    #[test]
    fn pattern5_preceding_sibling_with_name() {
        let html = r#"<a name="named-anchor"></a><h2>Named</h2>"#;
        assert_eq!(first_heading_id(html).as_deref(), Some("named-anchor"));
    }

    #[test]
    fn pattern5_skips_whitespace_text_nodes() {
        let html = "<a id=\"ws-anchor\"></a>\n  <h2>With Whitespace</h2>";
        assert_eq!(first_heading_id(html).as_deref(), Some("ws-anchor"));
    }

    #[test]
    fn no_id_returns_none() {
        let html = "<h2>No Anchor</h2>";
        assert_eq!(first_heading_id(html), None);
    }

    #[test]
    fn pattern2_sphinx_section_with_headerlink() {
        let html = r##"
            <section id="what-about-namespaces">
            <h3>What about namespaces?<a class="headerlink" href="#what-about-namespaces">¶</a></h3>
            <p>Details here.</p>
            </section>
        "##;
        assert_eq!(
            first_heading_id(html).as_deref(),
            Some("what-about-namespaces")
        );
    }
}

mod list_section_headers {
    use super::*;

    #[test]
    fn basic_headings() {
        let html = r#"
            <html><body>
                <h1 id="title">Title</h1>
                <p>Intro paragraph</p>
                <h2 id="setup">Setup</h2>
                <p>Setup instructions here</p>
                <h2 id="usage">Usage</h2>
                <p>Usage details</p>
            </body></html>
        "#;

        let headers = list_section_headers(html);
        assert_eq!(headers.len(), 3);

        assert_eq!(headers[0].id, "title");
        assert_eq!(headers[0].level, 1);
        assert_eq!(headers[0].text, "Title");

        assert_eq!(headers[1].id, "setup");
        assert_eq!(headers[1].level, 2);
        assert!(headers[1].preview.contains("Setup instructions"));

        assert_eq!(headers[2].id, "usage");
        assert_eq!(headers[2].level, 2);
    }

    #[test]
    fn skips_headings_without_ids() {
        let html = r#"
            <html><body>
                <h1>No ID Here</h1>
                <h2 id="with-id">Has ID</h2>
            </body></html>
        "#;

        let headers = list_section_headers(html);
        assert_eq!(headers.len(), 1);
        assert_eq!(headers[0].id, "with-id");
    }

    #[test]
    fn deduplicates_ids() {
        let html = r##"
            <html><body>
                <section id="dupe">
                    <h2>First <a href="#dupe">¶</a></h2>
                    <p>Content</p>
                </section>
            </body></html>
        "##;

        let headers = list_section_headers(html);
        assert_eq!(headers.len(), 1);
        assert_eq!(headers[0].id, "dupe");
    }

    #[test]
    fn strips_permalink_pilcrow() {
        let html = r##"<h2 id="sec">My Section<a href="#sec">¶</a></h2>"##;
        let headers = list_section_headers(html);
        assert_eq!(headers[0].text, "My Section");
    }

    #[test]
    fn empty_page_returns_empty() {
        let html = "<html><body><p>No headings here</p></body></html>";
        assert!(list_section_headers(html).is_empty());
    }
}

mod format_section_listing {
    use super::*;

    #[test]
    fn produces_xml_with_ids_and_levels() {
        let html = r#"
            <html><body>
                <h2 id="install">Installation</h2>
                <p>Run npm install</p>
                <h3 id="prereqs">Prerequisites</h3>
                <p>Node.js 18+</p>
            </body></html>
        "#;

        let listing = format_section_listing(html);

        assert!(listing.starts_with("<sections>"));
        assert!(listing.ends_with("</sections>"));
        assert!(listing.contains(r#"id="install""#));
        assert!(listing.contains(r#"level="2""#));
        assert!(listing.contains(r#"id="prereqs""#));
        assert!(listing.contains(r#"level="3""#));
        assert!(listing.contains("Installation"));
        assert!(listing.contains("Prerequisites"));
    }

    #[test]
    fn no_sections_message() {
        let html = "<html><body><p>Hello</p></body></html>";
        let listing = format_section_listing(html);
        assert_eq!(listing, "No sections with anchors found on this page.");
    }
}

mod extract_sections {
    use super::*;

    #[test]
    fn extracts_multiple_sections() {
        let html = r#"
            <html>
            <head><title>Docs</title></head>
            <body>
                <h1>Title</h1>
                <p>Intro</p>
                <h2 id="install">Installation</h2>
                <p>Install steps</p>
                <h2 id="usage">Usage</h2>
                <p>Usage info</p>
                <h2 id="api">API</h2>
                <p>API reference</p>
            </body>
            </html>
        "#;

        let result = extract_sections(html, &["install".to_owned(), "api".to_owned()]);

        assert!(result.contains("Install steps"));
        assert!(result.contains("API reference"));
        assert!(!result.contains("Usage info"));
        assert!(!result.contains("Intro"));
        assert!(result.contains("<title>Docs</title>"));
    }

    #[test]
    fn falls_back_to_full_html_when_no_ids_found() {
        let html = "<html><body><p>Hello</p></body></html>";
        let result = extract_sections(html, &["nonexistent".to_owned()]);
        assert!(result.contains("Hello"));
    }

    #[test]
    fn handles_section_wrapper_ids() {
        let html = r#"
            <html><body>
                <section id="config">
                    <h3>Configuration</h3>
                    <p>Config details</p>
                </section>
                <section id="deploy">
                    <h3>Deployment</h3>
                    <p>Deploy details</p>
                </section>
            </body></html>
        "#;

        let result = extract_sections(html, &["config".to_owned()]);
        assert!(result.contains("Config details"));
        assert!(!result.contains("Deploy details"));
    }
}

mod extract_preview_after_heading {
    use scraper::Html;

    use super::*;

    fn preview_for(html: &str) -> String {
        let doc = Html::parse_document(html);
        let sel = Selector::parse("h1, h2, h3, h4, h5, h6").unwrap();
        let heading = doc.select(&sel).next().unwrap();
        extract_preview_after_heading(&heading)
    }

    #[test]
    fn collects_following_paragraph() {
        let html = r#"<h2 id="s">Title</h2><p>Some preview text here.</p>"#;
        let preview = preview_for(html);
        assert_eq!(preview, "Some preview text here.");
    }

    #[test]
    fn stops_at_next_same_level_heading() {
        let html = r#"<h2 id="a">A</h2><p>Content A</p><h2 id="b">B</h2><p>Content B</p>"#;
        let preview = preview_for(html);
        assert!(preview.contains("Content A"));
        assert!(!preview.contains("Content B"));
    }

    #[test]
    fn truncates_long_preview() {
        let long_text = "word ".repeat(100);
        let html = format!(r#"<h2 id="s">Title</h2><p>{long_text}</p>"#);
        let preview = preview_for(&html);
        assert!(preview.len() <= PREVIEW_MAX + 3);
        assert!(preview.ends_with("..."));
    }

    #[test]
    fn empty_when_nothing_follows() {
        let html = r#"<h2 id="s">Title</h2>"#;
        let preview = preview_for(html);
        assert!(preview.is_empty());
    }
}
