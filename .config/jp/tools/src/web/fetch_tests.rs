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
