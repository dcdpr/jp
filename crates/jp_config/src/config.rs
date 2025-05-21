use confique::{meta::FieldKind, Config as Confique};

use crate::{conversation, editor, error::Result, llm, style, template};

/// Workspace Configuration.
#[derive(Debug, Clone, Default, PartialEq, Confique)]
pub struct Config {
    /// Inherit from a local ancestor or global configuration file.
    #[config(default = true)]
    pub inherit: bool,

    /// LLM-specific configuration.
    #[config(nested)]
    pub llm: llm::Config,

    /// Conversation-specific configuration.
    #[config(nested)]
    pub conversation: conversation::Config,

    /// Styling configuration.
    #[config(nested)]
    pub style: style::Config,

    /// Template configuration.
    #[config(nested)]
    pub template: template::Config,

    #[config(nested)]
    pub editor: editor::Config,
}

impl Config {
    #[must_use]
    pub fn fields() -> Vec<String> {
        let mut output = Vec::new();
        let mut stack = vec![(&Self::META, String::new())];

        while let Some((meta, prefix)) = stack.pop() {
            for field in meta.fields {
                let mut path = field.name.to_string();
                if !prefix.is_empty() {
                    path = format!("{prefix}.{path}");
                }

                if let FieldKind::Nested { meta } = field.kind {
                    stack.push((meta, path));
                } else {
                    output.push(path);
                }
            }
        }

        output
    }

    /// Set a configuration value using a stringified key/value pair.
    pub fn set(&mut self, path: &str, key: &str, value: impl Into<String>) -> Result<()> {
        match key {
            "inherit" => self.inherit = value.into().parse()?,
            _ if key.starts_with("llm.") => self.llm.set(path, &key[4..], value)?,
            _ if key.starts_with("style.") => self.style.set(path, &key[6..], value)?,
            _ if key.starts_with("conversation.") => {
                self.conversation.set(path, &key[13..], value)?;
            }
            _ if key.starts_with("template.") => self.template.set(path, &key[9..], value)?,
            _ if key.starts_with("editor.") => self.editor.set(path, &key[9..], value)?,
            _ => return crate::set_error(path, key),
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    #[test]
    fn test_set() {
        let cases = [
            ("inherit", "false", Config {
                inherit: false,
                ..Default::default()
            }),
            ("llm.provider.openrouter.api_key_env", "FOO", Config {
                llm: llm::Config {
                    provider: llm::provider::Config {
                        openrouter: llm::provider::openrouter::Config {
                            api_key_env: "FOO".to_owned(),
                            ..Default::default()
                        },
                        ..Default::default()
                    },
                    ..Default::default()
                },
                ..Default::default()
            }),
            ("style.code.file_link", "full", Config {
                style: style::Config {
                    code: style::code::Config {
                        file_link: style::code::LinkStyle::Full,
                        ..Default::default()
                    },
                },
                ..Default::default()
            }),
            ("conversation.title.generate.auto", "false", Config {
                conversation: conversation::Config {
                    title: conversation::title::Config {
                        generate: conversation::title::generate::Config {
                            auto: false,
                            ..Default::default()
                        },
                    },
                    ..Default::default()
                },
                ..Default::default()
            }),
            ("template.values.name", "\"Homer\"", Config {
                template: template::Config {
                    values: HashMap::from([("name".to_owned(), "Homer".into())]),
                },

                ..Default::default()
            }),
        ];

        for (key, value, expected) in cases {
            let mut config = Config::default();
            config.set(key, key, value).unwrap();
            assert_eq!(config, expected);
        }

        let mut config = Config::default();
        let err = config.set("", "invalid.key", "true").unwrap_err();
        assert!(err
            .to_string()
            .starts_with("Unknown config key: invalid.key\n\nAvailable keys:\n"));
    }
}
