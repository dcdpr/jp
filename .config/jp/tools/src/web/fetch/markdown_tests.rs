use url::Url;

use super::*;

fn url(s: &str) -> Url {
    Url::parse(s).unwrap()
}

mod to_markdown_url {
    use super::*;

    #[test]
    fn appends_md_to_simple_path() {
        let got = to_markdown_url(&url("https://docs.anthropic.com/en/api/rate-limits")).unwrap();
        assert_eq!(
            got.as_str(),
            "https://docs.anthropic.com/en/api/rate-limits.md"
        );
    }

    #[test]
    fn strips_trailing_slash() {
        let got = to_markdown_url(&url("https://docs.example.com/guide/")).unwrap();
        assert_eq!(got.as_str(), "https://docs.example.com/guide.md");
    }

    #[test]
    fn strips_html_suffix() {
        let got = to_markdown_url(&url("https://example.com/page.html")).unwrap();
        assert_eq!(got.as_str(), "https://example.com/page.md");
    }

    #[test]
    fn strips_htm_suffix() {
        let got = to_markdown_url(&url("https://example.com/old.htm")).unwrap();
        assert_eq!(got.as_str(), "https://example.com/old.md");
    }

    #[test]
    fn already_md_returns_none() {
        assert!(to_markdown_url(&url("https://example.com/page.md")).is_none());
    }

    #[test]
    fn root_path_returns_none() {
        assert!(to_markdown_url(&url("https://example.com/")).is_none());
        assert!(to_markdown_url(&url("https://example.com")).is_none());
    }

    #[test]
    fn preserves_query_and_fragment() {
        let got = to_markdown_url(&url("https://example.com/page?foo=1&bar=2#section")).unwrap();
        assert_eq!(
            got.as_str(),
            "https://example.com/page.md?foo=1&bar=2#section"
        );
    }
}

mod is_acceptable_markdown_content_type {
    use super::*;

    #[test]
    fn rejects_html() {
        assert!(!is_acceptable_markdown_content_type("text/html"));
        assert!(!is_acceptable_markdown_content_type(
            "text/html; charset=utf-8"
        ));
        assert!(!is_acceptable_markdown_content_type(
            "application/xhtml+xml"
        ));
    }

    #[test]
    fn accepts_common_markdown_types() {
        assert!(is_acceptable_markdown_content_type("text/markdown"));
        assert!(is_acceptable_markdown_content_type(
            "text/markdown; charset=utf-8"
        ));
        assert!(is_acceptable_markdown_content_type("text/x-markdown"));
        assert!(is_acceptable_markdown_content_type("text/plain"));
        assert!(is_acceptable_markdown_content_type("application/markdown"));
    }

    #[test]
    fn accepts_missing_content_type() {
        assert!(is_acceptable_markdown_content_type(""));
    }

    #[test]
    fn accepts_other_text_types() {
        assert!(is_acceptable_markdown_content_type("text/asciidoc"));
    }

    #[test]
    fn rejects_non_text_types() {
        assert!(!is_acceptable_markdown_content_type("application/json"));
        assert!(!is_acceptable_markdown_content_type("image/png"));
        assert!(!is_acceptable_markdown_content_type(
            "application/octet-stream"
        ));
    }
}

mod slugify {
    use super::*;

    #[test]
    fn basic_lowercasing() {
        assert_eq!(slugify("Hello World"), "hello-world");
    }

    #[test]
    fn strips_punctuation() {
        assert_eq!(slugify("Hello, World!"), "hello-world");
        assert_eq!(slugify("What's Up?"), "whats-up");
    }

    #[test]
    fn preserves_hyphens_and_underscores() {
        assert_eq!(slugify("already-slug_case"), "already-slug_case");
    }

    #[test]
    fn collapses_multi_whitespace() {
        assert_eq!(slugify("a   b\tc"), "a-b-c");
    }

    #[test]
    fn trims_leading_trailing_dashes() {
        assert_eq!(slugify("  hello  "), "hello");
        assert_eq!(slugify("--hello--"), "hello");
    }

    #[test]
    fn unicode_alphanumerics_preserved_and_lowercased() {
        assert_eq!(slugify("Über Café"), "über-café");
    }

    #[test]
    fn empty_for_pure_punctuation() {
        assert_eq!(slugify("!!!"), "");
    }
}

mod slugger {
    use super::*;

    #[test]
    fn deduplicates_repeated_slugs() {
        let mut s = Slugger::default();
        assert_eq!(s.slug("Setup"), "setup");
        assert_eq!(s.slug("Setup"), "setup-1");
        assert_eq!(s.slug("Setup"), "setup-2");
    }

    #[test]
    fn distinct_headings_not_affected() {
        let mut s = Slugger::default();
        assert_eq!(s.slug("Install"), "install");
        assert_eq!(s.slug("Usage"), "usage");
    }
}

mod atx_level {
    use super::*;

    #[test]
    fn levels_1_through_6() {
        assert_eq!(atx_level("# A"), Some(1));
        assert_eq!(atx_level("## B"), Some(2));
        assert_eq!(atx_level("### C"), Some(3));
        assert_eq!(atx_level("#### D"), Some(4));
        assert_eq!(atx_level("##### E"), Some(5));
        assert_eq!(atx_level("###### F"), Some(6));
    }

    #[test]
    fn too_many_hashes() {
        assert_eq!(atx_level("####### G"), None);
    }

    #[test]
    fn hash_without_space_is_not_heading() {
        // `#tag` is a hashtag, not a heading.
        assert_eq!(atx_level("#tag"), None);
    }

    #[test]
    fn just_hashes_is_heading() {
        // CommonMark allows empty headings.
        assert_eq!(atx_level("###"), Some(3));
    }

    #[test]
    fn non_heading() {
        assert_eq!(atx_level("paragraph"), None);
        assert_eq!(atx_level(""), None);
    }
}

mod parse_headings {
    use super::*;

    #[test]
    fn collects_atx_headings_with_slugs() {
        let md = "# Title\n\nIntro\n\n## Setup\n\nSetup text\n\n### Details\n\n## Usage\n";
        let h = parse_headings(md);
        assert_eq!(h.len(), 4);

        assert_eq!(h[0].level, 1);
        assert_eq!(h[0].slug, "title");
        assert_eq!(h[0].line, 0);

        assert_eq!(h[1].level, 2);
        assert_eq!(h[1].slug, "setup");

        assert_eq!(h[2].level, 3);
        assert_eq!(h[2].slug, "details");

        assert_eq!(h[3].level, 2);
        assert_eq!(h[3].slug, "usage");
    }

    #[test]
    fn ignores_headings_inside_backtick_fence() {
        let md = "# Real Heading\n\n```\n# Not A Heading\n```\n\n## Another\n";
        let h = parse_headings(md);
        assert_eq!(h.len(), 2);
        assert_eq!(h[0].slug, "real-heading");
        assert_eq!(h[1].slug, "another");
    }

    #[test]
    fn ignores_headings_inside_tilde_fence() {
        let md = "# Real\n\n~~~\n## Fake\n~~~\n\n## After\n";
        let h = parse_headings(md);
        assert_eq!(h.len(), 2);
        assert_eq!(h[0].slug, "real");
        assert_eq!(h[1].slug, "after");
    }

    #[test]
    fn strips_trailing_closing_hashes() {
        let md = "## Title ##\n";
        let h = parse_headings(md);
        assert_eq!(h[0].text, "Title");
        assert_eq!(h[0].slug, "title");
    }

    #[test]
    fn dedupes_repeated_slugs() {
        let md = "## Setup\n\ntext\n\n## Setup\n\nmore\n";
        let h = parse_headings(md);
        assert_eq!(h[0].slug, "setup");
        assert_eq!(h[1].slug, "setup-1");
    }
}

mod format_section_listing {
    use super::*;

    #[test]
    fn produces_xml_with_slugs_and_levels() {
        let md = "# Title\n\nIntro paragraph.\n\n## Setup\n\nInstall node.\n";
        let listing = format_section_listing(md);

        assert!(listing.starts_with("<sections>"));
        assert!(listing.ends_with("</sections>"));
        assert!(listing.contains(r#"id="title""#));
        assert!(listing.contains(r#"level="1""#));
        assert!(listing.contains(r#"id="setup""#));
        assert!(listing.contains(r#"level="2""#));
        assert!(listing.contains("Intro paragraph"));
        assert!(listing.contains("Install node"));
    }

    #[test]
    fn no_headings_returns_fallback_message() {
        assert_eq!(
            format_section_listing("Just a paragraph.\n"),
            "No sections with anchors found on this page."
        );
    }

    #[test]
    fn preview_stops_at_next_same_level_heading() {
        let md = "## A\n\nContent A.\n\n## B\n\nContent B.\n";
        let listing = format_section_listing(md);
        // Preview for A must not leak B's content.
        let a_start = listing.find(r#"id="a""#).unwrap();
        let b_start = listing.find(r#"id="b""#).unwrap();
        let a_slice = &listing[a_start..b_start];
        assert!(a_slice.contains("Content A"));
        assert!(!a_slice.contains("Content B"));
    }
}

mod extract_sections {
    use super::*;

    #[test]
    fn extracts_single_section() {
        let md = "# Intro\n\nintro text\n\n## Setup\n\nsetup text\n\n## Usage\n\nusage text\n";
        let got = extract_sections(md, &["setup".to_owned()]);
        assert!(got.contains("## Setup"));
        assert!(got.contains("setup text"));
        assert!(!got.contains("usage text"));
        assert!(!got.contains("intro text"));
    }

    #[test]
    fn extracts_multiple_sections_in_order() {
        let md = "## A\n\naaa\n\n## B\n\nbbb\n\n## C\n\nccc\n";
        let got = extract_sections(md, &["a".to_owned(), "c".to_owned()]);
        assert!(got.contains("aaa"));
        assert!(got.contains("ccc"));
        assert!(!got.contains("bbb"));
    }

    #[test]
    fn includes_nested_subsections() {
        let md = "## Parent\n\np\n\n### Child\n\nc\n\n## Sibling\n\ns\n";
        let got = extract_sections(md, &["parent".to_owned()]);
        assert!(got.contains("Parent"));
        assert!(got.contains("Child"));
        assert!(got.contains('c'));
        assert!(!got.contains("Sibling"));
    }

    #[test]
    fn unknown_id_returns_empty() {
        let md = "## Real\n\ntext\n";
        assert!(extract_sections(md, &["fake".to_owned()]).is_empty());
    }
}

mod process {
    use super::*;

    #[test]
    fn fragment_narrows_output() {
        let md = "# Top\n\ntop\n\n## Section\n\nsec text\n\n## Other\n\nother text\n";
        let out = process(md, &url("https://e.com/x#section"), false, None);
        assert!(out.contains("sec text"));
        assert!(!out.contains("other text"));
    }

    #[test]
    fn unresolved_fragment_falls_back_to_full_body() {
        let md = "# Only\n\nonly text\n";
        let out = process(md, &url("https://e.com/x#nope"), false, None);
        assert!(out.contains("only text"));
    }

    #[test]
    fn list_sections_wins_over_fragment() {
        let md = "## A\n\naaa\n\n## B\n\nbbb\n";
        let out = process(md, &url("https://e.com/x#a"), true, None);
        assert!(out.starts_with("<sections>"));
    }

    #[test]
    fn explicit_sections_used_over_fragment() {
        let md = "## A\n\naaa\n\n## B\n\nbbb\n";
        let ids = ["b".to_owned()];
        let out = process(md, &url("https://e.com/x#a"), false, Some(&ids));
        assert!(out.contains("bbb"));
        assert!(!out.contains("aaa"));
    }
}
