use super::*;

mod html_to_markdown {
    use super::*;

    #[test]
    fn strips_scripts_and_styles() {
        let html = r"
            <html><body>
                <style>body { color: red; }</style>
                <script>alert('xss')</script>
                <p>Keep this</p>
            </body></html>
        ";
        let md = html_to_markdown(html).unwrap();
        assert!(!md.contains("color: red"));
        assert!(!md.contains("alert"));
        assert!(md.contains("Keep this"));
    }

    #[test]
    fn converts_headings_and_links() {
        let html = "<h1>Title</h1><p><a href=\"https://example.com\">Link</a></p>";
        let md = html_to_markdown(html).unwrap();
        assert!(md.contains("# Title"));
        assert!(md.contains("[Link](https://example.com)"));
    }

    #[test]
    fn strips_svg_and_iframes() {
        let html = "<p>Before</p><svg><rect/></svg><iframe src=\"x\"></iframe><p>After</p>";
        let md = html_to_markdown(html).unwrap();
        assert!(!md.contains("rect"));
        assert!(!md.contains("iframe"));
        assert!(md.contains("Before"));
        assert!(md.contains("After"));
    }

    #[test]
    fn empty_body() {
        let html = "<html><head></head><body></body></html>";
        let md = html_to_markdown(html).unwrap();
        assert_eq!(md.trim(), "");
    }
}

mod is_binary {
    use super::*;

    #[test]
    fn binary_types() {
        assert!(is_binary("image/png"));
        assert!(is_binary("audio/mpeg"));
        assert!(is_binary("video/mp4"));
        assert!(is_binary("application/octet-stream"));
        assert!(is_binary("application/pdf"));
        assert!(is_binary("application/zip"));
    }

    #[test]
    fn text_types() {
        assert!(!is_binary("text/html; charset=utf-8"));
        assert!(!is_binary("text/plain"));
        assert!(!is_binary("application/json"));
    }
}

mod collapse_blank_lines {
    use super::*;

    #[test]
    fn preserves_single_and_double_newlines() {
        assert_eq!(collapse_blank_lines("a\nb\n\nc"), "a\nb\n\nc");
    }

    #[test]
    fn collapses_triple_plus() {
        assert_eq!(collapse_blank_lines("a\n\n\n\n\nb"), "a\n\nb");
    }

    #[test]
    fn trims_trailing_whitespace() {
        assert_eq!(collapse_blank_lines("hello\n\n\n"), "hello");
    }

    #[test]
    fn empty() {
        assert_eq!(collapse_blank_lines(""), "");
    }
}

mod canonical {
    use super::*;

    #[test]
    fn drops_the_fragment() {
        let url = Url::parse("https://example.com/page?b=2&a=1#section").unwrap();

        assert_eq!(
            HttpContent::<Https>::default()
                .canonical(&url)
                .unwrap()
                .as_str(),
            "https://example.com/page?b=2&a=1"
        );
    }
}
