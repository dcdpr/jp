use jp_tool::Outcome;

use super::*;

fn url(s: &str) -> Url {
    Url::parse(s).unwrap()
}

fn ids(ids: &[&str]) -> Vec<String> {
    ids.iter().map(|&id| id.to_owned()).collect()
}

fn content(result: ToolResult) -> String {
    match result.unwrap() {
        Outcome::Success { content } => content,
        other => panic!("expected success, got {other:?}"),
    }
}

fn error_message(result: ToolResult) -> String {
    match result.unwrap() {
        Outcome::Error { message, .. } => message,
        other => panic!("expected error, got {other:?}"),
    }
}

/// `pdftohtml -xml` output with the shape poppler 25.10 writes: positioned page
/// text with stray link text before it, then a nested outline.
const OUTLINE_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE pdf2xml SYSTEM "pdf2xml.dtd">

<pdf2xml producer="poppler" version="25.10.0">
 link to page 2  link to page 2 <page number="1" position="absolute" top="0" left="0" height="1262" width="892">
<text top="108" left="175" width="547" height="19" font="0">Attention &amp; Friends</text>
</page>
<outline>
<item page="1">Introduction</item>
<item page="2">Model Architecture</item>
<outline>
<item page="2">Encoder and Decoder Stacks</item>
<item>Attention</item>
<outline>
<item page="2">Scaled Dot-Product Attention</item>
</outline>
</outline>
<item page="2">Q&amp;A &#8211; Notes</item>
</outline>
</pdf2xml>
"#;

mod parse_outline {
    use super::*;

    #[test]
    fn reads_nested_items_in_document_order() {
        let items = parse_outline(OUTLINE_XML);

        assert_eq!(items, vec![
            OutlineItem {
                title: "Introduction".to_owned(),
                page: Some(1),
                depth: 1,
            },
            OutlineItem {
                title: "Model Architecture".to_owned(),
                page: Some(2),
                depth: 1,
            },
            OutlineItem {
                title: "Encoder and Decoder Stacks".to_owned(),
                page: Some(2),
                depth: 2,
            },
            OutlineItem {
                title: "Attention".to_owned(),
                page: None,
                depth: 2,
            },
            OutlineItem {
                title: "Scaled Dot-Product Attention".to_owned(),
                page: Some(2),
                depth: 3,
            },
            OutlineItem {
                title: "Q&A \u{2013} Notes".to_owned(),
                page: Some(2),
                depth: 1,
            },
        ]);
    }

    #[test]
    fn a_pdf_without_bookmarks_has_no_outline() {
        let xml = r#"<pdf2xml producer="poppler" version="25.10.0">
<page number="1"><text font="0">Hello</text></page>
</pdf2xml>"#;

        assert_eq!(parse_outline(xml), vec![]);
    }
}

mod resolve_pages {
    use super::*;

    fn item(title: &str, page: Option<usize>, depth: usize) -> OutlineItem {
        OutlineItem {
            title: title.to_owned(),
            page,
            depth,
        }
    }

    #[test]
    fn an_item_without_a_page_takes_the_next_one() {
        let items = vec![item("Parent", None, 1), item("Child", Some(3), 2)];

        assert_eq!(resolve_pages(items, 5), vec![
            Bookmark {
                title: "Parent".to_owned(),
                page: 3,
                level: 1,
            },
            Bookmark {
                title: "Child".to_owned(),
                page: 3,
                level: 2,
            },
        ]);
    }

    #[test]
    fn items_with_no_page_in_range_are_dropped() {
        let items = vec![
            item("Kept", Some(2), 1),
            item("Past the end", Some(9), 1),
            item("Trailing, no page", None, 1),
        ];

        assert_eq!(resolve_pages(items, 5), vec![Bookmark {
            title: "Kept".to_owned(),
            page: 2,
            level: 1,
        }]);
    }

    #[test]
    fn deep_items_are_capped_at_level_six() {
        let items = vec![item("Deep", Some(1), 9)];

        assert_eq!(resolve_pages(items, 1)[0].level, 6);
    }
}

mod split_pages {
    use super::*;

    #[test]
    fn the_trailing_form_feed_does_not_start_a_page() {
        assert_eq!(split_pages("one\n\u{c}two\n\u{c}"), vec!["one\n", "two\n"]);
    }

    #[test]
    fn a_blank_page_in_the_middle_is_kept() {
        assert_eq!(split_pages("one\u{c}\u{c}three\u{c}"), vec![
            "one", "", "three"
        ]);
    }

    #[test]
    fn empty_output_is_one_empty_page() {
        assert_eq!(split_pages(""), vec![""]);
    }
}

mod is_section_number {
    use super::*;

    #[test]
    fn accepts_numbered_and_appendix_forms() {
        assert!(is_section_number("3"));
        assert!(is_section_number("3.2.1"));
        assert!(is_section_number("5.7."));
        assert!(is_section_number("A.1"));
    }

    #[test]
    fn rejects_words() {
        assert!(!is_section_number(""));
        assert!(!is_section_number("."));
        assert!(!is_section_number("A"));
        assert!(!is_section_number("Figure"));
        assert!(!is_section_number("3 Model"));
    }
}

mod build {
    use super::*;

    #[test]
    fn a_numbered_heading_wins_over_an_earlier_figure_label() {
        // Page 4 of "Attention Is All You Need": the figure label repeats the
        // section title ten lines above the heading itself.
        let text = "Scaled Dot-Product Attention\nMulti-Head Attention\n\n3.2.1\n\nScaled \
                    Dot-Product Attention\n\nWe call our particular attention.\n\u{c}";
        let xml = r#"<outline><item page="1">Scaled Dot-Product Attention</item></outline>"#;

        let doc = Document::build(text, xml);

        assert_eq!(doc.pages, vec![
            "<p>Scaled Dot-Product Attention\nMulti-Head Attention</p>\n<h1 \
             id=\"scaled-dot-product-attention\">3.2.1 Scaled Dot-Product Attention</h1>\n<p>We \
             call our particular attention.</p>\n"
        ]);
        assert_eq!(doc.sections, vec!["scaled-dot-product-attention"]);
    }

    #[test]
    fn a_title_with_its_own_number_matches_the_line_exactly() {
        let text = "RFC 9562\n5.7.  UUID   Version 7\nUUIDv7 features a time-ordered value.\n\u{c}";
        let xml = r#"<outline><item page="1">5.7. UUID Version 7</item></outline>"#;

        let doc = Document::build(text, xml);

        assert_eq!(doc.pages, vec![
            "<p>RFC 9562</p>\n<h1 id=\"57-uuid-version-7\">5.7. UUID Version 7</h1>\n<p>UUIDv7 \
             features a time-ordered value.</p>\n"
        ]);
    }

    #[test]
    fn titles_are_searched_only_on_their_own_page() {
        // The table of contents on page 1 repeats the title; the bookmark
        // points at page 2, so the heading goes there.
        let text = "Table of Contents\n1. Introduction\n\u{c}1. Introduction\nBody.\n\u{c}";
        let xml = r#"<outline><item page="2">1. Introduction</item></outline>"#;

        let doc = Document::build(text, xml);

        assert_eq!(doc.pages, vec![
            "<p>Table of Contents\n1. Introduction</p>\n",
            "<h1 id=\"1-introduction\">1. Introduction</h1>\n<p>Body.</p>\n",
        ]);
    }

    #[test]
    fn a_title_not_found_goes_after_the_previous_heading() {
        let text = "Alpha\nFirst.\nSecond.\n\u{c}";
        let xml = r#"<outline>
<item page="1">Alpha</item>
<outline><item page="1">Wrapped over
two lines</item></outline>
</outline>"#;

        let doc = Document::build(text, xml);

        assert_eq!(doc.pages, vec![
            "<h1 id=\"alpha\">Alpha</h1>\n<h2 id=\"wrapped-over-two-lines\">Wrapped over two \
             lines</h2>\n<p>First.\nSecond.</p>\n"
        ]);
    }

    #[test]
    fn duplicate_titles_get_distinct_ids() {
        let text = "Notes\nOne.\n\nNotes\nTwo.\n\u{c}";
        let xml = r#"<outline><item page="1">Notes</item><item page="1">Notes</item></outline>"#;

        let doc = Document::build(text, xml);

        assert_eq!(doc.pages, vec![
            "<h1 id=\"notes\">Notes</h1>\n<p>One.</p>\n<h1 \
             id=\"notes-1\">Notes</h1>\n<p>Two.</p>\n"
        ]);
        assert_eq!(doc.sections, vec!["notes", "notes-1"]);
    }

    #[test]
    fn without_bookmarks_every_page_gets_a_heading() {
        let text = "First page.\n\u{c}Second <page> & more.\n\u{c}";

        let doc = Document::build(text, "<pdf2xml></pdf2xml>");

        assert_eq!(doc.pages, vec![
            "<h1 id=\"page-1\">Page 1</h1>\n<p>First page.</p>\n",
            "<h1 id=\"page-2\">Page 2</h1>\n<p>Second &lt;page&gt; &amp; more.</p>\n",
        ]);
        assert_eq!(doc.sections, Vec::<String>::new());
    }
}

mod parse_page_id {
    use super::*;

    #[test]
    fn single_pages_and_ranges() {
        assert_eq!(parse_page_id("page-3"), Some(3..=3));
        assert_eq!(parse_page_id("pages-2-5"), Some(2..=5));
        assert_eq!(parse_page_id("pages-4-4"), Some(4..=4));
    }

    #[test]
    fn malformed_ids_are_not_pages() {
        assert_eq!(parse_page_id("page-0"), None);
        assert_eq!(parse_page_id("page-x"), None);
        assert_eq!(parse_page_id("pages-5-2"), None);
        assert_eq!(parse_page_id("pages-0-2"), None);
        assert_eq!(parse_page_id("pages-3"), None);
        assert_eq!(parse_page_id("introduction"), None);
    }
}

/// Two pages with bookmarks "Introduction" on page 1, "Method" and its child
/// "Setup" on page 2.
fn sample() -> Document {
    let text = "Title of the paper\n\n1\n\nIntroduction\n\nWe study \
                things.\n\u{c}2\n\nMethod\n\nWe did things.\n\n2.1\n\nSetup\n\nWith care.\n\u{c}";
    let xml = r#"<outline>
<item page="1">Introduction</item>
<item page="2">Method</item>
<outline><item page="2">Setup</item></outline>
</outline>"#;

    Document::build(text, xml)
}

mod answer {
    use super::*;

    #[test]
    fn list_sections_names_bookmarks_and_the_page_count() {
        // The listing is the HTML pipeline's: a preview runs on until a
        // heading at the same or a higher level, so a parent's preview takes
        // in its children.
        let out = content(answer(
            &sample(),
            &url("https://example.com/a.pdf"),
            true,
            None,
        ));

        assert_eq!(
            out,
            "<sections>\n  <s id=\"introduction\" level=\"1\">1 Introduction - We study \
             things.</s>\n  <s id=\"method\" level=\"1\">2 Method - We did things. 2.1 Setup With \
             care.</s>\n  <s id=\"setup\" level=\"2\">2.1 Setup - With \
             care.</s>\n</sections>\n\nPage count: 2. Request pages as `page-N` or `pages-N-M`."
        );
    }

    #[test]
    fn a_section_includes_its_children_and_stops_at_its_sibling() {
        let out = content(answer(
            &sample(),
            &url("https://example.com/a.pdf"),
            false,
            Some(ids(&["method"])),
        ));

        assert_eq!(
            out,
            "# 2 Method\n\nWe did things.\n\n## 2.1 Setup\n\nWith care."
        );
    }

    #[test]
    fn pages_and_sections_come_back_in_request_order() {
        let out = content(answer(
            &sample(),
            &url("https://example.com/a.pdf"),
            false,
            Some(ids(&["setup", "page-1"])),
        ));

        assert_eq!(
            out,
            "## 2.1 Setup\n\nWith care.\n\nTitle of the paper\n\n# 1 Introduction\n\nWe study \
             things."
        );
    }

    #[test]
    fn a_page_fragment_narrows_to_that_page() {
        let out = content(answer(
            &sample(),
            &url("https://example.com/a.pdf#page=2&zoom=100"),
            false,
            None,
        ));

        assert_eq!(
            out,
            "# 2 Method\n\nWe did things.\n\n## 2.1 Setup\n\nWith care."
        );
    }

    #[test]
    fn a_section_fragment_narrows_to_that_section() {
        let out = content(answer(
            &sample(),
            &url("https://example.com/a.pdf#setup"),
            false,
            None,
        ));

        assert_eq!(out, "## 2.1 Setup\n\nWith care.");
    }

    #[test]
    fn an_unrecognized_fragment_returns_the_whole_document() {
        let out = content(answer(
            &sample(),
            &url("https://example.com/a.pdf#nameddest=intro"),
            false,
            None,
        ));

        assert_eq!(
            out,
            "Title of the paper\n\n# 1 Introduction\n\nWe study things.\n\n# 2 Method\n\nWe did \
             things.\n\n## 2.1 Setup\n\nWith care."
        );
    }

    #[test]
    fn unknown_ids_are_an_error_not_the_whole_document() {
        let message = error_message(answer(
            &sample(),
            &url("https://example.com/a.pdf"),
            false,
            Some(ids(&["method", "nope", "also-nope"])),
        ));

        assert_eq!(
            message,
            "No section `nope`, `also-nope` in this PDF. Call with `list_sections` to see its \
             section ids; pages are addressable as `page-N` and `pages-N-M`."
        );
    }

    #[test]
    fn a_page_past_the_end_is_an_error() {
        let message = error_message(answer(
            &sample(),
            &url("https://example.com/a.pdf"),
            false,
            Some(ids(&["pages-2-3"])),
        ));

        assert_eq!(
            message,
            "`pages-2-3` is past the end of this PDF, which has 2 pages."
        );
    }

    #[test]
    fn a_large_document_is_cut_short_and_followed_by_the_listing() {
        // Enough pages that the markdown exceeds the threshold.
        let page = format!("{}\n\u{c}", "word ".repeat(1_000));
        let doc = Document::build(&page.repeat(50), "<pdf2xml></pdf2xml>");

        let out = content(answer(&doc, &url("https://example.com/a.pdf"), false, None));

        // The listing is the part that lets the caller ask for less next
        // time, so it must survive the cut whole.
        assert!(out.len() <= SUMMARIZE_THRESHOLD + 200, "{}", out.len());
        assert!(out.starts_with("# Page 1\n\nword word"));
        assert!(out.contains("[Content truncated: showing "));
        assert!(out.contains(&format!("\n\n{TOO_LARGE}\n\n<sections>\n")));
        assert!(
            out.ends_with(
                "</sections>\n\nPage count: 50. Request pages as `page-N` or `pages-N-M`."
            )
        );
    }
}

mod detection {
    use super::*;

    #[test]
    fn pdf_content_types() {
        assert!(is_pdf_content_type("application/pdf"));
        assert!(is_pdf_content_type("Application/PDF; qs=0.001"));
        assert!(is_pdf_content_type("application/x-pdf"));
        assert!(!is_pdf_content_type("application/octet-stream"));
        assert!(!is_pdf_content_type("text/html"));
    }

    #[test]
    fn pdf_paths() {
        assert!(has_pdf_path(&url("https://example.com/rfc/rfc9562.pdf")));
        assert!(has_pdf_path(&url(
            "https://example.com/PAPER.PDF?dl=1#page=2"
        )));
        assert!(!has_pdf_path(&url("https://arxiv.org/pdf/1706.03762v7")));
        assert!(!has_pdf_path(&url("https://example.com/pdf/")));
    }

    #[test]
    fn signature_near_the_start() {
        assert!(has_pdf_signature(b"%PDF-1.7\n..."));
        assert!(has_pdf_signature(b"\xef\xbb\xbf junk %PDF-1.4"));
        assert!(!has_pdf_signature(b"PK\x03\x04zipfile"));
        assert!(!has_pdf_signature(
            &[b"x".repeat(1024), b"%PDF-1.4".to_vec()].concat()
        ));
    }
}
