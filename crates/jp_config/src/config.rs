use std::path::PathBuf;

use confique::{meta::FieldKind, Config as Confique};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    assignment::{set_error, AssignKeyValue, KvAssignment},
    assistant, conversation, editor,
    error::Result,
    is_default, is_empty, style, template, Partial,
};

pub type PartialConfig = <Config as Confique>::Partial;

/// Workspace Configuration.
#[derive(Debug, Clone, PartialEq, Confique, Serialize, Deserialize)]
#[config(partial_attr(derive(Debug, Clone, PartialEq, Serialize)))]
#[config(partial_attr(serde(deny_unknown_fields)))]
pub struct Config {
    /// Inherit from a local ancestor or global configuration file.
    #[config(
        default = true,
        partial_attr(serde(skip_serializing_if = "is_default"))
    )]
    pub inherit: bool,

    /// Paths from which configuration files can be loaded without specifying
    /// the full path to the file.
    ///
    /// Relative paths are resolved relative to the workspace root.
    ///
    /// Files in these paths are NOT loaded by default, but can instead be
    /// referenced by their basename and without a file extension. For example,
    /// a file named `my-agent.toml` in a config load path can be loaded using
    /// `--cfg my-agent`.
    #[config(default = [".jp/config.d"], partial_attr(serde(skip_serializing_if = "is_default")))]
    pub config_load_paths: Vec<PathBuf>,

    /// LLM-specific configuration.
    #[config(nested, partial_attr(serde(skip_serializing_if = "is_empty")))]
    pub assistant: assistant::Assistant,

    /// Conversation-specific configuration.
    #[config(nested, partial_attr(serde(skip_serializing_if = "is_empty")))]
    pub conversation: conversation::Conversation,

    /// Styling configuration.
    #[config(nested, partial_attr(serde(skip_serializing_if = "is_empty")))]
    pub style: style::Style,

    /// Template configuration.
    #[config(nested, partial_attr(serde(skip_serializing_if = "is_empty")))]
    pub template: template::Template,

    /// Editor configuration.
    #[config(nested, partial_attr(serde(skip_serializing_if = "is_empty")))]
    pub editor: editor::Editor,
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

    pub fn set_from_envs() -> Result<PartialConfig> {
        let mut partial = PartialConfig::empty();

        let envs = std::env::vars_os()
            .filter_map(|(k, v)| k.into_string().ok().map(|k| (k, v)))
            .filter_map(|(k, v)| v.into_string().ok().map(|v| (k, v)))
            .filter_map(|(k, v)| k.strip_prefix("JP_").map(|k| (k.to_owned(), v)))
            .map(|(k, v)| (k.to_ascii_lowercase(), v.clone()));

        for (key, value) in envs {
            let assignment = KvAssignment::try_from_env(&key, &value)?;
            partial.assign(assignment)?;
        }

        Ok(partial)
    }
}

impl AssignKeyValue for <Config as Confique>::Partial {
    fn assign(&mut self, mut kv: KvAssignment) -> Result<()> {
        let k = kv.key().as_str().to_owned();

        match k.as_str() {
            "inherit" => self.inherit = Some(kv.try_into_bool()?),
            "config_load_paths" => kv.try_set_or_merge_vec(
                self.config_load_paths.get_or_insert_default(),
                |v| match v {
                    Value::String(v) => Ok(PathBuf::from(v)),
                    _ => Err("Expected string".into()),
                },
            )?,
            "assistant" => self.assistant = kv.try_into_object()?,
            "conversation" => self.conversation = kv.try_into_object()?,
            "style" => self.style = kv.try_into_object()?,
            "template" => self.template = kv.try_into_object()?,
            "editor" => self.editor = kv.try_into_object()?,

            _ if kv.trim_prefix("assistant") => self.assistant.assign(kv)?,
            _ if kv.trim_prefix("conversation") => self.conversation.assign(kv)?,
            _ if kv.trim_prefix("style") => self.style.assign(kv)?,
            _ if kv.trim_prefix("template") => self.template.assign(kv)?,
            _ if kv.trim_prefix("editor") => self.editor.assign(kv)?,

            _ => return set_error(kv.key()),
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr as _;

    use confique::Partial as _;

    use super::*;

    #[test]
    fn test_set_cli() {
        let cases = [
            ("inherit=false", {
                let mut partial = PartialConfig::default_values();
                partial.inherit = Some(false);
                partial
            }),
            ("assistant.provider.openrouter.api_key_env=FOO", {
                let mut partial = PartialConfig::default_values();
                partial.assistant.provider.openrouter.api_key_env = Some("FOO".to_owned());
                partial
            }),
            ("style.code.file_link=full", {
                let mut partial = PartialConfig::default_values();
                partial.style.code.file_link = Some(style::code::LinkStyle::Full);
                partial
            }),
            ("conversation.title.generate.auto=false", {
                let mut partial = PartialConfig::default_values();
                partial.conversation.title.generate.auto = Some(false);
                partial
            }),
            ("template.values.name:=\"Homer\"", {
                let mut partial = PartialConfig::default_values();
                partial
                    .template
                    .values
                    .get_or_insert_default()
                    .insert("name".to_owned(), "Homer".into());
                partial
            }),
            ("config_load_paths=foo", {
                let mut partial = PartialConfig::default_values();
                partial.config_load_paths = Some(vec![PathBuf::from("foo")]);
                partial
            }),
            ("config_load_paths+=bar", {
                let mut partial = PartialConfig::default_values();
                partial.config_load_paths =
                    Some(vec![PathBuf::from(".jp/config.d"), PathBuf::from("bar")]);
                partial
            }),
        ];

        for (kv, expected) in cases {
            let mut config = PartialConfig::default_values();
            let kv = KvAssignment::from_str(kv).unwrap();

            config.assign(kv).unwrap();
            assert_eq!(config, expected);
        }

        let mut config = PartialConfig::default_values();
        let kv = KvAssignment::try_from_cli("invalid.key", "true").unwrap();

        let err = config.assign(kv).unwrap_err();
        assert!(err
            .to_string()
            .starts_with("Unknown config key: invalid.key\n\nAvailable keys:\n"));
    }
}
