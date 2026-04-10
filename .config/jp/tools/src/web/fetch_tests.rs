use super::*;

mod is_binary {
    use super::*;

    #[test]
    fn image_types() {
        assert!(is_binary("image/png"));
        assert!(is_binary("image/jpeg"));
        assert!(is_binary("Image/PNG"));
    }

    #[test]
    fn audio_video() {
        assert!(is_binary("audio/mpeg"));
        assert!(is_binary("video/mp4"));
    }

    #[test]
    fn application_types() {
        assert!(is_binary("application/octet-stream"));
        assert!(is_binary("application/pdf"));
        assert!(is_binary("application/zip"));
    }

    #[test]
    fn text_types_are_not_binary() {
        assert!(!is_binary("text/html; charset=utf-8"));
        assert!(!is_binary("text/plain"));
        assert!(!is_binary("application/json"));
        assert!(!is_binary("application/xml"));
    }
}

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

mod truncate {
    use super::*;

    #[test]
    fn under_limit_unchanged() {
        let s = "short content";
        assert_eq!(truncate(s, 100), s);
    }

    #[test]
    fn over_limit_truncates_with_note() {
        let s = "a".repeat(50);
        let result = truncate(&s, 20);
        assert!(result.starts_with("aaaaaaaaaaaaaaaaaaaa"));
        assert!(result.contains("[Content truncated:"));
        assert!(result.contains("20 of 50 bytes"));
    }

    #[test]
    fn exact_limit() {
        let s = "exactly10!";
        assert_eq!(truncate(s, 10), s);
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

        // Should contain the targeted section
        assert!(result.contains("<h2"), "should contain the h2 heading");
        assert!(result.contains("Setup"), "should contain heading text");
        assert!(
            result.contains("Setup instructions"),
            "should contain section content"
        );
        assert!(result.contains("example code"), "should contain code block");

        // Should NOT contain other sections
        assert!(!result.contains("Introduction"), "should not contain h1");
        assert!(!result.contains("Intro text"), "should not contain intro");
        assert!(
            !result.contains("Usage text"),
            "should not contain next section"
        );

        // Should preserve the original <head>
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

        // h2#errors is same level, should be excluded
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

        // This shouldn't panic even with weird id values
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
