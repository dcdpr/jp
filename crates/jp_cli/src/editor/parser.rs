use std::fmt;

const CONVERSATION_MARKER: &str = "\n<!-- CONVERSATION_MARKER -->\n";
const CONFIG_HEADER: &str = "\n# Active Configuration\n";
pub(super) const CUT_MARKER: &str = indoc::indoc!(
    "
    ---------------------------------------8<---------------------------------------
    --------------------- EVERYTHING BELOW THIS LINE IS IGNORED --------------------
    --------------------------------------->8---------------------------------------"
);

#[derive(Debug, PartialEq)]
pub enum Error {
    MissingConfigHeader,
    MissingConfigCodeBlock,
}

#[derive(Debug, PartialEq, Default)]
pub struct QueryDocument<'s> {
    pub query: &'s str,
    pub meta: QueryMetaSection<'s>,
}

#[derive(Debug, PartialEq, Default)]
pub struct QueryMetaSection<'s> {
    pub config: QueryConfigSection<'s>,
    pub history: QueryHistorySection<'s>,
}

#[derive(Debug, PartialEq, Default)]
pub struct QueryConfigSection<'s> {
    pub value: &'s str,
    pub error: Option<&'s str>,
}

#[derive(Debug, PartialEq, Default)]
pub struct QueryHistorySection<'s> {
    pub value: &'s str,
}

impl fmt::Display for QueryDocument<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.query)?;

        if self.meta != QueryMetaSection::default() {
            f.write_str("\n\n")?;
            f.write_str(CUT_MARKER)?;
            f.write_str("\n")?;

            write!(f, "{}", self.meta)?;
        }

        Ok(())
    }
}

impl From<QueryDocument<'_>> for String {
    fn from(doc: QueryDocument<'_>) -> Self {
        doc.to_string()
    }
}

impl fmt::Display for QueryMetaSection<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.config)?;
        if !self.history.value.trim().is_empty() {
            write!(f, "{}{}", CONVERSATION_MARKER, self.history)?;
        }

        Ok(())
    }
}

impl fmt::Display for QueryConfigSection<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("\n# Active Configuration\n\n")?;

        if let Some(error) = self.error {
            if error.starts_with("> ERROR:") {
                writeln!(f, "{error}")?;
            } else {
                let mut lines = error.lines();
                let first = lines.next().unwrap_or("");
                writeln!(f, "> ERROR: {first}")?;
                for line in lines {
                    writeln!(f, "> {line}")?;
                }
            }
        } else {
            indoc::writedoc!(
                f,
                "
                > NOTE: You can edit this configuration to apply it to the current conversation.
                >       If the configuration is invalid, the editor will re-open.
                >
                >       Only the most relevant configuration is shown here, but you can add any
                >       configuration properties you want to apply.
                "
            )?;
        }

        f.write_str("\n```toml\n")?;
        f.write_str(self.value)?;
        if !self.value.is_empty() && !self.value.ends_with('\n') {
            f.write_str("\n")?;
        }
        f.write_str("```\n")?;

        Ok(())
    }
}

impl fmt::Display for QueryHistorySection<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.value)
    }
}

impl<'s> TryFrom<&'s str> for QueryDocument<'s> {
    type Error = Error;

    fn try_from(content: &'s str) -> Result<Self, Self::Error> {
        let (query, tail) = content.split_once(CUT_MARKER).unwrap_or((content, ""));

        let meta = if tail.trim().is_empty() {
            QueryMetaSection::default()
        } else {
            QueryMetaSection::try_from(tail)?
        };

        Ok(QueryDocument {
            query: query.trim(),
            meta,
        })
    }
}

impl<'s> TryFrom<&'s str> for QueryMetaSection<'s> {
    type Error = Error;

    fn try_from(s: &'s str) -> Result<Self, Self::Error> {
        let (config, tail) = s.rsplit_once(CONVERSATION_MARKER).unwrap_or((s, ""));
        let config = QueryConfigSection::try_from(config)?;
        let history = QueryHistorySection { value: tail.trim() };

        Ok(Self { config, history })
    }
}

impl<'s> TryFrom<&'s str> for QueryConfigSection<'s> {
    type Error = Error;

    fn try_from(s: &'s str) -> Result<Self, Self::Error> {
        // Remove the header.
        let s = s
            .split_once(CONFIG_HEADER)
            .map(|(_, b)| b.trim())
            .ok_or(Error::MissingConfigHeader)?;

        // Find the error message, if any.
        let mut n = 0;
        let error = if s.starts_with("> ERROR:") {
            for (i, c) in s.char_indices() {
                if c == '\n'
                    && s.get(i + c.len_utf8()..)
                        .is_none_or(|v| !v.starts_with('>'))
                {
                    break;
                }

                n = i + c.len_utf8();
            }

            Some(s[..n].trim())
        } else {
            None
        };

        // Get the config code block content.
        let value = s[n..]
            .trim()
            .split_once("```toml")
            .and_then(|(_, b)| b.rsplit_once("```").map(|(a, _)| a.trim()))
            .ok_or(Error::MissingConfigCodeBlock)?
            .trim();

        Ok(Self { value, error })
    }
}

#[cfg(test)]
mod tests {
    use indoc::indoc;
    use pretty_assertions::assert_eq;

    use super::*;

    #[test]
    #[expect(clippy::too_many_lines)]
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

                    > NOTE: You can edit this configuration to apply it to the current \
                     conversation.
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
    #[expect(clippy::too_many_lines)]
    fn test_query_document() {
        struct TestCase {
            input: &'static str,
            want: Result<QueryDocument<'static>, Error>,
        }

        let cases = vec![
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
}
