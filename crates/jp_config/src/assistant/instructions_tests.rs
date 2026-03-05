use super::*;

#[test]
fn test_instructions_assign() {
    let mut p = PartialInstructionsConfig::default();

    let kv = KvAssignment::try_from_cli("title", "foo").unwrap();
    p.assign(kv).unwrap();
    assert_eq!(p.title, Some("foo".into()));

    let kv = KvAssignment::try_from_cli("description", "bar").unwrap();
    p.assign(kv).unwrap();
    assert_eq!(p.description, Some("bar".into()));

    let kv = KvAssignment::try_from_cli("items", "baz").unwrap();
    p.assign(kv).unwrap();
    assert_eq!(p.items, Some(vec!["baz".into()]));

    let kv = KvAssignment::try_from_cli("items+", "quux").unwrap();
    p.assign(kv).unwrap();
    assert_eq!(p.items, Some(vec!["baz".into(), "quux".into()]));

    let kv = KvAssignment::try_from_cli("items.0", "quuz").unwrap();
    p.assign(kv).unwrap();
    assert_eq!(p.items, Some(vec!["quuz".into(), "quux".into()]));

    let kv = KvAssignment::try_from_cli("examples", "qux").unwrap();
    p.assign(kv).unwrap();
    assert_eq!(p.examples, vec![PartialExampleConfig::Generic(
        "qux".into()
    )]);

    let kv = KvAssignment::try_from_cli("examples+", "quuz").unwrap();
    p.assign(kv).unwrap();
    assert_eq!(p.examples, vec![
        PartialExampleConfig::Generic("qux".into()),
        PartialExampleConfig::Generic("quuz".into())
    ]);

    let kv = KvAssignment::try_from_cli("examples.0", "quuz").unwrap();
    p.assign(kv).unwrap();
    assert_eq!(p.examples, vec![
        PartialExampleConfig::Generic("quuz".into()),
        PartialExampleConfig::Generic("quuz".into())
    ]);
}

#[test]
fn test_example_assign() {
    let mut p = PartialExampleConfig::default();

    let kv = KvAssignment::try_from_cli("", "bar").unwrap();
    p.assign(kv).unwrap();
    assert_eq!(p, PartialExampleConfig::Generic("bar".into()));

    let kv = KvAssignment::try_from_cli(":", r#""bar""#).unwrap();
    p.assign(kv).unwrap();
    assert_eq!(p, PartialExampleConfig::Generic("bar".into()));

    let kv = KvAssignment::try_from_cli(":", r#"{"good":"bar","bad":"baz"}"#).unwrap();
    p.assign(kv).unwrap();
    assert_eq!(
        p,
        PartialExampleConfig::Contrast(PartialContrastConfig {
            good: Some("bar".into()),
            bad: Some("baz".into()),
            reason: None,
        })
    );

    let kv = KvAssignment::try_from_cli("nope", "nope").unwrap();
    assert_eq!(&p.assign(kv).unwrap_err().to_string(), "nope: unknown key");
}

#[test]
fn test_contrast_assign() {
    let mut p = PartialContrastConfig::default();

    let kv = KvAssignment::try_from_cli("good", "bar").unwrap();
    p.assign(kv).unwrap();
    assert_eq!(p, PartialContrastConfig {
        good: Some("bar".into()),
        bad: None,
        reason: None,
    });

    let kv = KvAssignment::try_from_cli("bad", "baz").unwrap();
    p.assign(kv).unwrap();
    assert_eq!(p, PartialContrastConfig {
        good: Some("bar".into()),
        bad: Some("baz".into()),
        reason: None,
    });

    let kv = KvAssignment::try_from_cli("reason", "qux").unwrap();
    p.assign(kv).unwrap();
    assert_eq!(p, PartialContrastConfig {
        good: Some("bar".into()),
        bad: Some("baz".into()),
        reason: Some("qux".into()),
    });

    let kv = KvAssignment::try_from_cli(":", r#"{"good":"one","bad":null}"#).unwrap();
    p.assign(kv).unwrap();
    assert_eq!(p, PartialContrastConfig {
        good: Some("one".into()),
        bad: None,
        reason: None,
    });

    let kv = KvAssignment::try_from_cli("nope", "nope").unwrap();
    assert_eq!(&p.assign(kv).unwrap_err().to_string(), "nope: unknown key");
}

#[test]
fn test_to_section_basic() {
    let i = InstructionsConfig {
        title: Some("Guidelines".to_owned()),
        description: None,
        position: 0,
        items: vec!["Be concise".to_owned(), "Be clear".to_owned()],
        examples: vec![],
    };

    let section = i.to_section();
    assert_eq!(section.tag.as_deref(), Some("instruction"));
    assert_eq!(section.title.as_deref(), Some("Guidelines"));
    assert_eq!(section.content, "- Be concise\n- Be clear");
    assert_eq!(
        section.render(),
        "<instruction title=\"Guidelines\">\n- Be concise\n- Be clear\n</instruction>"
    );
}

#[test]
fn test_to_section_with_description_and_examples() {
    let i = InstructionsConfig {
        title: Some("foo".to_owned()),
        description: Some("bar".to_owned()),
        position: 0,
        items: vec!["foo".to_owned(), "bar".to_owned()],
        examples: vec![
            ExampleConfig::Generic("example one".to_owned()),
            ExampleConfig::Contrast(ContrastConfig {
                good: "good".to_owned(),
                bad: "bad".to_owned(),
                reason: Some("because".to_owned()),
            }),
            ExampleConfig::Contrast(ContrastConfig {
                good: "quux".to_owned(),
                bad: "quuz".to_owned(),
                reason: None,
            }),
        ],
    };

    let section = i.to_section();
    insta::assert_snapshot!(section.render());
}

#[test]
fn test_to_section_cdata() {
    let i = InstructionsConfig {
        title: Some("foo".to_owned()),
        description: None,
        position: 0,
        items: vec!["bar <test>bar</test>".to_owned()],
        examples: vec![],
    };

    let section = i.to_section();
    insta::assert_snapshot!(section.render());
}
