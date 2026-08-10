use super::*;
use crate::Status;

#[test]
fn vue_interpolation_is_neutralised() {
    assert_eq!(
        escape("see {{ config }} for this"),
        "see &#123;&#123; config &#125;&#125; for this"
    );
    assert_eq!(escape("{{{a}}}"), "&#123;&#123;{a&#125;&#125;}");
}

#[test]
fn tag_like_angle_brackets_are_escaped() {
    assert_eq!(
        escape("<script>alert(1)</script>"),
        "&lt;script>alert(1)&lt;/script>"
    );
    assert_eq!(escape("<!-- comment -->"), "&lt;!-- comment -->");
}

/// Prose and arithmetic keep their angle brackets: only something that could
/// parse as a tag is a hazard.
#[test]
fn bare_angle_brackets_survive() {
    assert_eq!(escape("width < indent"), "width < indent");
    assert_eq!(escape("a <= b > c"), "a <= b > c");
    assert_eq!(escape("2 <3"), "2 <3");
}

#[test]
fn ordinary_markdown_is_untouched() {
    let body = "# Heading\n\n- a list\n- with `code` and **bold**\n\n```rust\nlet x = 1;\n```\n";

    assert_eq!(escape(body), body);
}

#[test]
fn each_source_names_its_own_metadata_field() {
    let issue = Source::GitHub { number: 123 };
    let elsewhere = Source::external("bear:E340A2C4-8671-4233-860B-6AEFF7CB00D8").unwrap();

    assert_eq!(issue.field(), "GitHub");
    assert_eq!(issue.marker(), "#123");
    assert_eq!(elsewhere.field(), "Source");
    assert_eq!(
        elsewhere.marker(),
        "bear:E340A2C4-8671-4233-860B-6AEFF7CB00D8"
    );
}

/// An id may carry colons of its own; only the first one divides the pair.
#[test]
fn a_source_splits_on_its_first_colon() {
    assert_eq!(
        Source::external("bear:a:b:c").unwrap().marker(),
        "bear:a:b:c"
    );
    assert_eq!(
        Source::external("  bear : 42 ").unwrap().marker(),
        "bear:42"
    );
}

#[test]
fn a_source_that_is_not_a_pair_is_refused() {
    assert_eq!(
        Source::external("nocolon"),
        Err(SourceError::NotAPair("nocolon".to_owned()))
    );
    assert_eq!(
        Source::external("bear:"),
        Err(SourceError::NotAPair("bear:".to_owned()))
    );
    assert_eq!(
        Source::external(":42"),
        Err(SourceError::NotAPair(":42".to_owned()))
    );
    assert_eq!(
        Source::external("my source:42"),
        Err(SourceError::Scheme("my source".to_owned()))
    );
}

/// A marker is written to the metadata block unescaped, so anything that could
/// break out of a line — or out of the page the site renders — is refused
/// rather than mangled.
#[test]
fn a_marker_that_could_escape_its_line_is_refused() {
    assert_eq!(
        Source::external("bear:a\nb"),
        Err(SourceError::Id {
            id: "a\nb".to_owned(),
            char_: '\n'
        })
    );
    assert_eq!(
        Source::external("bear:<script>"),
        Err(SourceError::Id {
            id: "<script>".to_owned(),
            char_: '<'
        })
    );
    assert_eq!(
        Source::external("bear:{{ x }}"),
        Err(SourceError::Id {
            id: "{{ x }}".to_owned(),
            char_: '{'
        })
    );
}

/// A ticket imported from one source must not be mistaken for one imported from
/// another, however the markers happen to line up.
#[test]
fn a_source_only_links_a_ticket_carrying_its_own_marker() {
    let metadata = Metadata {
        status: Status::Todo,
        kind: Kind::Bug,
        authors: "jean".to_owned(),
        date: "2026-08-05".to_owned(),
        blocked_by: None,
        implements: None,
        promoted_to: None,
        github: Some("#123".to_owned()),
        source: None,
    };

    assert!(Source::GitHub { number: 123 }.links(&metadata));
    assert!(!Source::GitHub { number: 7 }.links(&metadata));
    assert!(!Source::external("github:123").unwrap().links(&metadata));
}

#[test]
fn every_field_is_escaped() {
    let import = Import {
        source: Source::GitHub { number: 1 },
        title: "<script>t</script>",
        description: "{{ d }}",
        comments: vec![Comment {
            from: "gh:someone".to_owned(),
            date: "2026-08-05T00:00:00Z".to_owned(),
            re: None,
            body: "<div>hi</div>".to_owned(),
        }],
        kind: Kind::Bug,
        authors: "gh:someone",
        date: "2026-08-05",
    };

    let (title, description, comments) = escaped(&import);

    assert_eq!(title, "&lt;script>t&lt;/script>");
    assert_eq!(description, "&#123;&#123; d &#125;&#125;");
    assert_eq!(comments[0].body, "&lt;div>hi&lt;/div>");
    assert_eq!(comments[0].from, "gh:someone");
}
