use super::*;

#[test]
fn test_section_assign() {
    let mut p = PartialSectionConfig::default();

    let kv = KvAssignment::try_from_cli("tag", "foo").unwrap();
    p.assign(kv).unwrap();
    assert_eq!(p.tag, Some("foo".into()));

    let kv = KvAssignment::try_from_cli("title", "bar").unwrap();
    p.assign(kv).unwrap();
    assert_eq!(p.title, Some("bar".into()));

    let kv = KvAssignment::try_from_cli("content", "baz").unwrap();
    p.assign(kv).unwrap();
    assert_eq!(p.content, Some("baz".into()));

    let kv = KvAssignment::try_from_cli("position", "1").unwrap();
    p.assign(kv).unwrap();
    assert_eq!(p.position, Some(1));
}

#[test]
fn test_render_tag_and_title() {
    let section = SectionConfig {
        content: "- Be concise\n- Be clear".into(),
        tag: Some("instruction".into()),
        title: Some("Guidelines".into()),
        position: 0,
    };
    assert_eq!(
        section.render(),
        "<instruction title=\"Guidelines\">\n- Be concise\n- Be clear\n</instruction>"
    );
}

#[test]
fn test_render_tag_only() {
    let section = SectionConfig {
        content: "some content".into(),
        tag: Some("context".into()),
        title: None,
        position: 0,
    };
    assert_eq!(section.render(), "<context>\nsome content\n</context>");
}

#[test]
fn test_render_title_only() {
    let section = SectionConfig {
        content: "some content".into(),
        tag: None,
        title: Some("My Section".into()),
        position: 0,
    };
    assert_eq!(section.render(), "# My Section\n\nsome content");
}

#[test]
fn test_render_no_tag_no_title() {
    let section = SectionConfig {
        content: "raw content".into(),
        tag: None,
        title: None,
        position: 0,
    };
    assert_eq!(section.render(), "raw content");
}

#[test]
fn test_render_cdata_wrapping() {
    let section = SectionConfig {
        content: "foo <bar> baz".into(),
        tag: Some("data".into()),
        title: None,
        position: 0,
    };
    insta::assert_snapshot!(
        section.render(),
        @r"
        <data>
        <![CDATA[
        foo <bar> baz
        ]]>
        </data>
        "
    );
}

#[test]
fn test_render_cdata_with_end_marker() {
    let section = SectionConfig {
        content: "a]]>b".into(),
        tag: Some("data".into()),
        title: None,
        position: 0,
    };
    insta::assert_snapshot!(
        section.render(),
        @r"
        <data>
        <![CDATA[
        a]]]]><![CDATA[>b
        ]]>
        </data>
        "
    );
}

#[test]
fn test_render_attr_escaping() {
    let section = SectionConfig {
        content: "content".into(),
        tag: Some("s".into()),
        title: Some(r#"He said "hello" & goodbye"#.into()),
        position: 0,
    };
    assert_eq!(
        section.render(),
        "<s title=\"He said &quot;hello&quot; &amp; goodbye\">\ncontent\n</s>"
    );
}

#[test]
fn test_escape_attr() {
    assert_eq!(escape_attr("hello"), "hello");
    assert_eq!(escape_attr("a&b"), "a&amp;b");
    assert_eq!(escape_attr(r#"a"b"#), "a&quot;b");
    assert_eq!(escape_attr("a<b>c"), "a&lt;b&gt;c");
}

#[test]
fn test_wrap_cdata_if_needed() {
    assert!(matches!(wrap_cdata_if_needed("plain"), Cow::Borrowed(_)));
    assert_eq!(
        wrap_cdata_if_needed("<b>bold</b>").as_ref(),
        "<![CDATA[\n<b>bold</b>\n]]>"
    );
}
