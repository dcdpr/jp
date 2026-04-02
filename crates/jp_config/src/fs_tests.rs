use indoc::indoc;
use pretty_assertions::assert_eq;
use test_log::test;

use super::*;

#[test]
fn test_expand_tilde() {
    struct TestCase {
        path: &'static str,
        home: Option<&'static str>,
        expected: Option<&'static str>,
    }

    let cases = vec![
        ("no tilde with home", TestCase {
            path: "no/tilde/here",
            home: Some("/tmp"),
            expected: Some("no/tilde/here"),
        }),
        ("no tilde missing home", TestCase {
            path: "no/tilde/here",
            home: None,
            expected: Some("no/tilde/here"),
        }),
        ("tilde path with home", TestCase {
            path: "~/subdir",
            home: Some("/tmp"),
            expected: Some("/tmp/subdir"),
        }),
        ("only tilde with home", TestCase {
            path: "~",
            home: Some("/tmp"),
            expected: Some("/tmp"),
        }),
        ("tilde missing home", TestCase {
            path: "~",
            home: None,
            expected: None,
        }),
    ];

    for (name, case) in cases {
        assert_eq!(
            expand_tilde(case.path, case.home),
            case.expected.map(Utf8PathBuf::from),
            "Failed test case: {name}"
        );
    }
}

fn merge(target: &str, source: &str) -> String {
    let mut target_doc: toml_edit::DocumentMut = target.parse().unwrap();
    let source_doc: toml_edit::DocumentMut = source.parse().unwrap();
    deep_merge_toml(target_doc.as_table_mut(), source_doc.as_table());
    target_doc.to_string()
}

#[test]
fn deep_merge_adds_new_top_level_key() {
    let result = merge("existing = 1\n", "new_key = 2\n");
    assert_eq!(result, "existing = 1\nnew_key = 2\n");
}

#[test]
fn deep_merge_overwrites_existing_value() {
    let result = merge("key = \"old\"\n", "key = \"new\"\n");
    assert_eq!(result, "key = \"new\"\n");
}

#[test]
fn deep_merge_preserves_untouched_keys() {
    let target = indoc! {r#"
        first = "a"
        second = "b"
        third = "c"
    "#};
    let source = "second = \"B\"\n";
    let result = merge(target, source);
    assert_eq!(result, indoc! {r#"
        first = "a"
        second = "B"
        third = "c"
    "#});
}

#[test]
fn deep_merge_preserves_comments() {
    let target = indoc! {r#"
        # This is a comment
        key = "value"

        [section]
        # Another comment
        nested = true
    "#};
    let source = "[section]\nnew_key = false\n";
    let result = merge(target, source);
    assert!(result.contains("# This is a comment"));
    assert!(result.contains("# Another comment"));
    assert!(result.contains("nested = true"));
    assert!(result.contains("new_key = false"));
}

#[test]
fn deep_merge_recurses_into_nested_tables() {
    let target = indoc! {r#"
        [parent]
        keep = "this"
        change = "old"
    "#};
    let source = indoc! {r#"
        [parent]
        change = "new"
    "#};
    let result = merge(target, source);
    assert_eq!(result, indoc! {r#"
        [parent]
        keep = "this"
        change = "new"
    "#});
}

#[test]
fn deep_merge_adds_new_nested_key() {
    let target = indoc! {"
        [parent]
        existing = 1
    "};
    let source = indoc! {"
        [parent]
        added = 2
    "};
    let result = merge(target, source);
    assert_eq!(result, indoc! {"
        [parent]
        existing = 1
        added = 2
    "});
}

#[test]
fn deep_merge_preserves_key_order() {
    let target = indoc! {"
        z_last = 1
        a_first = 2
        m_middle = 3
    "};
    let source = "a_first = 99\n";
    let result = merge(target, source);
    assert_eq!(result, indoc! {"
        z_last = 1
        a_first = 99
        m_middle = 3
    "});
}

#[test]
fn deep_merge_deeply_nested() {
    let target = indoc! {r#"
        [a.b.c]
        deep = "old"
        untouched = true
    "#};
    let source = indoc! {r#"
        [a.b.c]
        deep = "new"
    "#};
    let result = merge(target, source);
    assert_eq!(result, indoc! {r#"
        [a.b.c]
        deep = "new"
        untouched = true
    "#});
}

#[test]
fn deep_merge_replaces_value_with_table() {
    let target = "key = \"string\"\n";
    let source = "[key]\nnested = true\n";
    let result = merge(target, source);
    assert!(result.contains("[key]"));
    assert!(result.contains("nested = true"));
}

#[test]
fn deep_merge_replaces_table_with_value() {
    let target = "[key]\nnested = true\n";
    let source = "key = \"string\"\n";
    let result = merge(target, source);
    assert_eq!(result, "key = \"string\"\n");
}

#[test]
fn merge_delta_preserves_formatting() {
    let original = indoc! {r#"
        extends = [
            "config/personas/default.toml",
            "mcp/tools/**/*.toml",
        ]

        [providers.llm.aliases]
        anthropic = "anthropic/claude-sonnet-4-6"
        haiku = "anthropic/claude-haiku-4-5"

        [style.code]
        copy_link = "osc8"
    "#};

    let mut config = ConfigFile {
        path: "test.toml".into(),
        format: Format::Toml,
        content: original.to_owned(),
    };

    // A partial that only sets conversation.default_id
    let mut delta = PartialAppConfig::default();
    delta.conversation.default_id = Some(crate::conversation::DefaultConversationId::Ask);

    config.merge_delta(&delta).unwrap();

    // Original content should be preserved
    assert!(config.content.contains(r"extends = ["), "extends preserved");
    assert!(
        config
            .content
            .contains(r#"anthropic = "anthropic/claude-sonnet-4-6""#),
        "aliases preserved as strings, not expanded to tables"
    );
    assert!(
        config.content.contains(r#"copy_link = "osc8""#),
        "style preserved"
    );

    // New value should be present
    assert!(
        config.content.contains(r#"default_id = "ask""#),
        "delta applied: {}",
        config.content
    );
}

#[test]
fn merge_delta_into_empty_file() {
    let mut config = ConfigFile {
        path: "test.toml".into(),
        format: Format::Toml,
        content: String::new(),
    };

    let mut delta = PartialAppConfig::default();
    delta.conversation.start_local = Some(true);

    config.merge_delta(&delta).unwrap();
    assert!(
        config.content.contains("start_local = true"),
        "{}",
        config.content
    );
}

#[test]
fn merge_delta_rejects_non_toml() {
    let mut config = ConfigFile {
        path: "test.json".into(),
        format: Format::Json,
        content: "{}".to_owned(),
    };

    let delta = PartialAppConfig::default();
    let result = config.merge_delta(&delta);
    assert!(result.is_err());
}
