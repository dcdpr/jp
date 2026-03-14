use indoc::indoc;
use pretty_assertions::assert_eq;
use test_log::test;

use super::*;

#[test]
fn test_query_config_section() {
    struct TestCase {
        input: &'static str,
        want: Result<QueryConfigSection<'static>, Error>,
    }

    let cases = vec![
        ("missing header", TestCase {
            input: "No header",
            want: Err(Error::MissingConfigHeader),
        }),
        ("missing code block", TestCase {
            input: "\n# Active Configuration\n",
            want: Err(Error::MissingConfigCodeBlock),
        }),
        ("valid", TestCase {
            input: indoc!(
                r#"

                    # Active Configuration

                    > NOTE: You can edit this configuration to apply it to the current conversation.
                    >       If the configuration is invalid, the editor will re-open.
                    >
                    >       Only the most relevant configuration is shown here, but you can add any
                    >       configuration properties you want to apply.

                    ```toml
                    [assistant.model.id]
                    provider = "ollama"
                    name = "bar"
                    ```
                    "#
            ),
            want: Ok(QueryConfigSection {
                value: indoc!(
                    r#"
                        [assistant.model.id]
                        provider = "ollama"
                        name = "bar""#
                ),
                error: None,
            }),
        }),
        ("nested code block", TestCase {
            input: indoc!(
                r#"

                    # Active Configuration

                    > NOTE: You can edit this configuration to apply it to the current conversation.
                    >       If the configuration is invalid, the editor will re-open.
                    >
                    >       Only the most relevant configuration is shown here, but you can add any
                    >       configuration properties you want to apply.

                    ```toml
                    [assistant.model.id]
                    provider = "ollama"
                    name = """
                    ```toml
                    test
                    ```
                    """
                    ```
                    "#
            ),
            want: Ok(QueryConfigSection {
                value: indoc!(
                    r#"
                        [assistant.model.id]
                        provider = "ollama"
                        name = """
                        ```toml
                        test
                        ```
                        """"#
                ),
                error: None,
            }),
        }),
        ("with error", TestCase {
            input: indoc!(
                r#"

                    # Active Configuration

                    > ERROR: Configuration parsing error
                    >
                    > foo

                    ```toml
                    [assistant.model.id]
                    provider = "ollama"
                    name = "bar"
                    ```
                    "#
            ),
            want: Ok(QueryConfigSection {
                value: indoc!(
                    r#"
                        [assistant.model.id]
                        provider = "ollama"
                        name = "bar""#
                ),
                error: Some("> ERROR: Configuration parsing error\n>\n> foo"),
            }),
        }),
    ];

    for (name, case) in cases {
        let result = QueryConfigSection::try_from(case.input);
        assert_eq!(result, case.want, "failed case: {name}");
    }
}

#[test]
fn test_query_config_section_display() {
    struct TestCase {
        input: QueryConfigSection<'static>,
        want: &'static str,
    }

    let cases = vec![
        ("empty", TestCase {
            input: QueryConfigSection {
                value: "",
                error: None,
            },
            want: indoc!(
                "

                # Active Configuration

                > NOTE: You can edit this configuration to apply it to the current conversation.
                >       If the configuration is invalid, the editor will re-open.
                >
                >       Only the most relevant configuration is shown here, but you can add any
                >       configuration properties you want to apply.

                ```toml
                ```
                "
            ),
        }),
        ("with error", TestCase {
            input: QueryConfigSection {
                value: "",
                error: Some("> ERROR: Configuration parsing error\n>\n> foo"),
            },
            want: indoc!(
                "

                    # Active Configuration

                    > ERROR: Configuration parsing error
                    >
                    > foo

                    ```toml
                    ```
                    "
            ),
        }),
        ("with config", TestCase {
            input: QueryConfigSection {
                value: indoc!(
                    r#"
                        [assistant]
                        model.id = "openrouter"
                        model.parameters.reasoning.effort = "low"
                        "#
                ),
                error: None,
            },
            want: indoc!(
                r#"

                    # Active Configuration

                    > NOTE: You can edit this configuration to apply it to the current conversation.
                    >       If the configuration is invalid, the editor will re-open.
                    >
                    >       Only the most relevant configuration is shown here, but you can add any
                    >       configuration properties you want to apply.

                    ```toml
                    [assistant]
                    model.id = "openrouter"
                    model.parameters.reasoning.effort = "low"
                    ```
                    "#
            ),
        }),
    ];

    for (name, case) in cases {
        assert_eq!(case.input.to_string(), case.want, "failed case: {name}");
    }
}

#[test]
fn test_query_document() {
    struct TestCase {
        input: &'static str,
        want: Result<QueryDocument<'static>, Error>,
    }

    let cases =
        vec![
            ("empty", TestCase {
                input: "",
                want: Ok(QueryDocument {
                    query: "",
                    meta: QueryMetaSection::default(),
                }),
            }),
            ("with query", TestCase {
                input: "foo",
                want: Ok(QueryDocument {
                    query: "foo",
                    meta: QueryMetaSection::default(),
                }),
            }),
            ("with meta", TestCase {
                input: indoc!(
                    r#"
                    foo

                    ---------------------------------------8<---------------------------------------
                    --------------------- EVERYTHING BELOW THIS LINE IS IGNORED --------------------
                    --------------------------------------->8---------------------------------------

                    # Active Configuration

                    > NOTE: You can edit this configuration to apply it to the current conversation.
                    >       If the configuration is invalid, the editor will re-open.
                    >
                    >       Only the most relevant configuration is shown here, but you can add any
                    >       configuration properties you want to apply.

                    ```toml
                    [assistant]
                    model.id = "openrouter"
                    model.parameters.reasoning.effort = "low"
                    ```
                    "#
                ),
                want: Ok(QueryDocument {
                    query: "foo",
                    meta: QueryMetaSection {
                        config: QueryConfigSection {
                            value: indoc!(
                                r#"
                                [assistant]
                                model.id = "openrouter"
                                model.parameters.reasoning.effort = "low""#
                            ),
                            error: None,
                        },
                        history: QueryHistorySection { value: "" },
                    },
                }),
            }),
            ("with meta and error", TestCase {
                input: indoc!(
                    r#"
                    foo

                    ---------------------------------------8<---------------------------------------
                    --------------------- EVERYTHING BELOW THIS LINE IS IGNORED --------------------
                    --------------------------------------->8---------------------------------------

                    # Active Configuration

                    > ERROR: Configuration parsing error
                    >
                    > foo

                    ```toml
                    [assistant]
                    model.id = "openrouter"
                    model.parameters.reasoning.effort = "low"
                    ```
                    "#
                ),
                want: Ok(QueryDocument {
                    query: "foo",
                    meta: QueryMetaSection {
                        config: QueryConfigSection {
                            value: indoc!(
                                r#"
                                [assistant]
                                model.id = "openrouter"
                                model.parameters.reasoning.effort = "low""#
                            ),
                            error: Some("> ERROR: Configuration parsing error\n>\n> foo"),
                        },
                        history: QueryHistorySection { value: "" },
                    },
                }),
            }),
            ("with history", TestCase {
                input: indoc!(
                    "
                    foo

                    ---------------------------------------8<---------------------------------------
                    --------------------- EVERYTHING BELOW THIS LINE IS IGNORED --------------------
                    --------------------------------------->8---------------------------------------

                    # Active Configuration

                    > NOTE: You can edit this configuration to apply it to the current conversation.
                    >       If the configuration is invalid, the editor will re-open.
                    >
                    >       Only the most relevant configuration is shown here, but you can add any
                    >       configuration properties you want to apply.

                    ```toml
                    ```

                    <!-- CONVERSATION_MARKER -->
                    # Conversation History

                    bar"
                ),
                want: Ok(QueryDocument {
                    query: "foo",
                    meta: QueryMetaSection {
                        config: QueryConfigSection {
                            value: "",
                            error: None,
                        },
                        history: QueryHistorySection {
                            value: "# Conversation History\n\nbar",
                        },
                    },
                }),
            }),
        ];

    for (name, case) in cases {
        let result = QueryDocument::try_from(case.input);
        assert_eq!(result, case.want, "failed case: {name}");

        if let Ok(result) = result {
            assert_eq!(
                result.to_string(),
                case.input,
                "failed case (roundtrip): {name}"
            );
        }
    }
}
