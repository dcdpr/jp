use super::*;

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
fn the_marker_links_a_ticket_to_its_issue() {
    let import = Import {
        number: 123,
        title: "t",
        description: "d",
        comments: vec![],
        kind: Kind::Bug,
        authors: "gh:someone",
        date: "2026-08-05",
    };

    assert_eq!(import.marker(), "#123");
}

#[test]
fn every_field_is_escaped() {
    let import = Import {
        number: 1,
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
