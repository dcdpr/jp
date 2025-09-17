//! The configuration types for Jean-Pierre.

#![warn(
    clippy::all,
    clippy::allow_attributes,
    clippy::cargo,
    clippy::missing_docs_in_private_items,
    clippy::nursery,
    clippy::pedantic,
    clippy::renamed_function_params,
    clippy::tests_outside_test_module,
    clippy::todo,
    clippy::try_err,
    clippy::unimplemented,
    clippy::unneeded_field_pattern,
    clippy::unseparated_literal_suffix,
    clippy::unused_result_ok,
    clippy::unused_trait_names,
    clippy::use_debug,
    clippy::unwrap_used,
    missing_docs,
    rustdoc::all,
    unused_doc_comments
)]
#![allow(
    clippy::derive_partial_eq_without_eq,
    reason = "schematic derives PartialEq but not Eq. We *could* do \
              `#[config(partial(derive(Eq))]`, but it's not worth it."
)]
#![allow(
    rustdoc::private_intra_doc_links,
    reason = "we don't host the docs, and use them mainly for LSP integration"
)]
// Should stabilize soon, see: <https://github.com/rust-lang/rust/pull/137487>
#![cfg_attr(test, feature(assert_matches))]

pub mod assignment;
pub mod assistant;
pub mod conversation;
pub mod editor;
pub mod error;
pub mod fs;
pub mod model;
pub mod providers;
pub mod style;
pub mod template;
pub mod util; // TODO: Rename

pub use error::Error;
use relative_path::RelativePathBuf;
use schematic::{Config, PartialConfig as _};
use serde_json::Value;

use crate::{
    assignment::{missing_key, type_error, AssignKeyValue, AssignResult, KvAssignment},
    assistant::{AssistantConfig, PartialAssistantConfig},
    conversation::{ConversationConfig, PartialConversationConfig},
    editor::{EditorConfig, PartialEditorConfig},
    providers::{PartialProviderConfig, ProviderConfig},
    style::{PartialStyleConfig, StyleConfig},
    template::{PartialTemplateConfig, TemplateConfig},
};

/// The prefix to use for environment variables that set configuration options.
///
/// The prefix contains the `CFG_` part to allow jp-related non-configuration
/// environment variables to be set, e.g. `JP_EDITOR` or `JP_GITHUB_TOKEN`, etc.
pub const ENV_PREFIX: &str = "JP_CFG_";

/// Convenience type for boxed errors.
type BoxedError = Box<dyn std::error::Error + Send + Sync>;

/// The global configuration for Jean Pierre.
#[derive(Debug, Config)]
#[config(rename_all = "snake_case")]
pub struct AppConfig {
    /// Inherit from a local ancestor or global configuration file.
    #[setting(default = true)]
    pub inherit: bool,

    /// Paths from which configuration files can be loaded without specifying
    /// the full path to the file.
    ///
    /// Paths are relative to both the workspace root, and the user's workspace
    /// override directory.
    ///
    /// For example, a path of `.jp/config.d` will be resolved to
    /// `<workspace-path>/.jp/config.d`, and
    /// `$XDG_DATA_HOME/jp/workspace/<workspace-id>/.jp/config.d`.
    ///
    /// If a file exists at both locations, the user's workspace file will be
    /// merged on top of the workspace file.
    ///
    /// Files in these paths are **NOT** loaded by default, but can instead be
    /// referenced by their basename, optionally without a file extension. For
    /// example, a file named `my-agent.toml` in a config load path can be
    /// loaded using `--cfg my-agent`.
    #[setting(default = vec![], merge = schematic::merge::append_vec)]
    pub config_load_paths: Vec<RelativePathBuf>,

    /// Extends the configuration from the given files.
    ///
    /// Paths are relative to the current config file.
    ///
    /// Files are allowed to be glob patterns, and will be expanded to a list
    /// of files to extend.
    ///
    /// Note that extended files ARE loaded by default, in contrast to
    /// [`Self::config_load_paths`].
    #[setting(default = vec!["config.d/**/*".into()], merge = schematic::merge::preserve)]
    pub extends: Vec<RelativePathBuf>,

    /// Assistant configuration.
    ///
    /// The assistant is the component that takes user input, and uses an LLM to
    /// generate a response. This configuration allows you to tweak the
    /// assistant, such as the name of the assistant, which LLM model the
    /// assistant should use, the system prompt to use, or specific instructions
    /// for the assistant.
    #[setting(nested)]
    pub assistant: AssistantConfig,

    /// Conversation configuration.
    ///
    /// Contains configuration specific to conversation management, such as
    /// (automated) title generation.
    #[setting(nested)]
    pub conversation: ConversationConfig,

    /// Style configuration.
    ///
    /// Contains configuration specific to output formatting, such as code
    /// blocks, reasoning, tool calls, etc.
    #[setting(nested)]
    pub style: StyleConfig,

    /// Editor configuration.
    ///
    /// Tweak how Jean-Pierre opens files for editing, for example when editing
    /// a message to send to the assistant. This allows you to use your own
    /// editor of choice, optionally with custom startup options, etc.
    #[setting(nested)]
    pub editor: EditorConfig,

    /// Template configuration.
    #[setting(nested)]
    pub template: TemplateConfig,

    /// Providers configuration.
    ///
    /// Providers are used to provide functionality to Jean-Pierre, such as
    /// different llm providers that are used by the assistant, or context
    /// providers used to augment assistant queries with additional context
    /// (e.g. file attachments, or tool call results).
    #[setting(nested)]
    pub providers: ProviderConfig,
}

impl AssignKeyValue for PartialAppConfig {
    fn assign(&mut self, mut kv: KvAssignment) -> AssignResult {
        match kv.key_string().as_str() {
            "inherit" => self.inherit = kv.try_some_bool()?,
            _ if kv.p("config_load_paths") => {
                let parser = |kv: KvAssignment| match kv.value.clone().into_value() {
                    Value::String(v) => Ok(RelativePathBuf::from(v)),
                    _ => type_error(kv.key(), &kv.value, &["string"]).map_err(Into::into),
                };

                kv.try_vec(self.config_load_paths.get_or_insert_default(), parser)?;
            }
            _ if kv.p("assistant") => self.assistant.assign(kv)?,
            _ if kv.p("conversation") => self.conversation.assign(kv)?,
            _ if kv.p("style") => self.style.assign(kv)?,
            _ if kv.p("editor") => self.editor.assign(kv)?,
            _ if kv.p("template") => self.template.assign(kv)?,
            _ if kv.p("providers") => self.providers.assign(kv)?,
            _ => return missing_key(&kv),
        }

        Ok(())
    }
}

impl AppConfig {
    /// Return a list of all fields in the configuration.
    ///
    /// The fields are returned in alphabetical order, with nested fields
    /// separated by a dot.
    ///
    /// ```rust
    /// # use jp_config::AppConfig;
    ///
    /// assert_eq!(&AppConfig::fields()[0..5], [
    ///     "config_load_paths",
    ///     "extends",
    ///     "inherit",
    ///     "template.values",
    ///     "style.typewriter.code_delay",
    /// ]);
    /// ```
    #[must_use]
    pub fn fields() -> Vec<String> {
        use schematic::{SchemaBuilder, SchemaType, Schematic as _};

        let builder = SchemaBuilder::default();
        let mut output = Vec::new();
        let mut stack = vec![(Self::build_schema(builder), String::new())];

        while let Some((schema, prefix)) = stack.pop() {
            let fields = match schema.ty {
                SchemaType::Struct(v) => v.fields,
                _ => break,
            };

            for (name, field) in fields {
                let mut path = name;
                if !prefix.is_empty() {
                    path = format!("{prefix}.{path}");
                }

                match field.schema.ty {
                    SchemaType::Struct(_) => stack.push((field.schema, path)),
                    _ => output.push(path),
                }
            }
        }

        output
    }
}

impl PartialAppConfig {
    /// Create a new empty partial configuration.
    #[expect(clippy::missing_panics_doc)]
    #[must_use]
    pub fn empty() -> Self {
        Self {
            inherit: None,
            config_load_paths: None,
            extends: None,
            assistant: PartialAssistantConfig::empty().expect("always works for non-enum types"),
            conversation: PartialConversationConfig::empty()
                .expect("always works for non-enum types"),
            style: PartialStyleConfig::empty().expect("always works for non-enum types"),
            editor: PartialEditorConfig::empty().expect("always works for non-enum types"),
            template: PartialTemplateConfig::empty().expect("always works for non-enum types"),
            providers: PartialProviderConfig::empty().expect("always works for non-enum types"),
        }
    }

    /// Create a new partial configuration from environment variables.
    ///
    /// # Errors
    ///
    /// Returns an error if the value of an environment variable is not a valid
    /// for the given field.
    pub fn from_envs() -> Result<Self, BoxedError> {
        let mut partial = Self::empty();

        let envs = std::env::vars_os()
            .filter_map(|(k, v)| k.into_string().ok().map(|k| (k, v)))
            .filter_map(|(k, v)| v.into_string().ok().map(|v| (k, v)))
            .filter_map(|(k, v)| k.strip_prefix(ENV_PREFIX).map(|k| (k.to_owned(), v)))
            .map(|(k, v)| (k.to_ascii_lowercase(), v));

        for (key, value) in envs {
            let assignment = KvAssignment::try_from_env(&key, &value)?;
            partial.assign(assignment)?;
        }

        Ok(partial)
    }
}

#[cfg(test)]
mod tests {
    use std::assert_matches::assert_matches;

    use schematic::PartialConfig as _;

    use super::*;
    use crate::assignment::{KvAssignmentError, KvAssignmentErrorKind};

    #[test]
    fn test_partial_app_config_empty_serialize() {
        insta::assert_debug_snapshot!(PartialAppConfig::empty());
    }

    #[test]
    fn test_partial_app_config_default_values() {
        insta::assert_debug_snapshot!(PartialAppConfig::default_values(&()));
    }

    #[test]
    fn test_partial_app_config_default() {
        insta::assert_debug_snapshot!(PartialAppConfig::default());
    }

    #[test]
    fn test_app_config_fields() {
        insta::assert_debug_snapshot!(AppConfig::fields());
    }

    #[test]
    fn test_ensure_no_missing_assignments() {
        // Some fields cannot be assigned via CLI.
        let skip_fields = ["extends"];

        for field in AppConfig::fields() {
            if skip_fields.contains(&field.as_str()) {
                continue;
            }

            let mut p = PartialAppConfig::default();
            let kv = KvAssignment::try_from_cli(&field, "foo").unwrap();
            if let Err(error) = p.assign(kv) {
                let Ok(error) = error.downcast::<KvAssignmentError>() else {
                    continue;
                };

                match &error.error {
                    KvAssignmentErrorKind::KvParse { .. }
                    | KvAssignmentErrorKind::UnknownKey { .. }
                    | KvAssignmentErrorKind::UnknownIndex { .. } => {}

                    KvAssignmentErrorKind::Json(_)
                    | KvAssignmentErrorKind::Parse { .. }
                    | KvAssignmentErrorKind::Type { .. }
                    | KvAssignmentErrorKind::ParseBool(_)
                    | KvAssignmentErrorKind::ParseInt(_)
                    | KvAssignmentErrorKind::ParseFloat(_) => continue,
                }

                panic!("unexpected error for field '{field}': {error:?}");
            }
        }
    }

    #[test]
    fn test_partial_app_config_assign() {
        let mut p = PartialAppConfig::default();

        let kv = KvAssignment::try_from_cli("inherit", "true").unwrap();
        p.assign(kv).unwrap();
        assert_eq!(p.inherit, Some(true));

        let kv = KvAssignment::try_from_cli("config_load_paths", "foo,bar").unwrap();
        p.assign(kv).unwrap();
        assert_eq!(p.config_load_paths, Some(vec!["foo".into(), "bar".into()]));

        let kv = KvAssignment::try_from_cli("assistant.name", "foo").unwrap();
        p.assign(kv).unwrap();
        assert_eq!(p.assistant.name.as_deref(), Some("foo"));

        let kv =
            KvAssignment::try_from_cli("assistant:", r#"{"name":"bar","system_prompt":"baz"}"#)
                .unwrap();
        p.assign(kv).unwrap();
        assert_eq!(p.assistant.name.as_deref(), Some("bar"));
        assert_eq!(p.assistant.system_prompt.as_deref(), Some("baz"));

        let kv = KvAssignment::try_from_cli("config_load_paths:", "[true]").unwrap();
        let error = p
            .assign(kv)
            .unwrap_err()
            .downcast::<KvAssignmentError>()
            .unwrap()
            .error;

        assert_matches!(
            error,
            KvAssignmentErrorKind::Type { need, .. } if need == ["string"]
        );
    }
}
