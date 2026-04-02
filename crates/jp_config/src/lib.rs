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
#![expect(
    clippy::derive_partial_eq_without_eq,
    reason = "schematic derives PartialEq but not Eq. We *could* do \
              `#[config(partial(derive(Eq))]`, but it's not worth it."
)]
#![expect(
    rustdoc::private_intra_doc_links,
    reason = "we don't host the docs, and use them mainly for LSP integration"
)]

pub mod assignment;
pub mod assistant;
pub mod conversation;
mod delta;
pub mod editor;
pub mod error;
pub mod fs;
pub(crate) mod internal;
pub mod model;
mod partial;
pub mod providers;
pub mod style;
pub mod template;
pub mod types;
pub mod util; // TODO: Rename

use std::sync::Arc;

pub use error::Error;
use indexmap::IndexMap;
pub use partial::ToPartial;
use relative_path::RelativePathBuf;
pub use schematic::{
    Config, ConfigError, PartialConfig, Schema, SchemaBuilder, SchemaType, Schematic, schema,
};
use serde_json::Value;

use crate::{
    assignment::{AssignKeyValue, AssignResult, KvAssignment, missing_key, type_error},
    assistant::{AssistantConfig, PartialAssistantConfig},
    conversation::{ConversationConfig, PartialConversationConfig},
    delta::{PartialConfigDelta, delta_opt_vec},
    editor::{EditorConfig, PartialEditorConfig},
    partial::partial_opt,
    providers::{PartialProviderConfig, ProviderConfig},
    style::{PartialStyleConfig, StyleConfig},
    template::{PartialTemplateConfig, TemplateConfig},
    types::extending_path::ExtendingRelativePath,
};

/// The prefix to use for environment variables that set configuration options.
///
/// The prefix contains the `CFG_` part to allow jp-related non-configuration
/// environment variables to be set, e.g. `JP_EDITOR` or `JP_GITHUB_TOKEN`, etc.
pub const ENV_PREFIX: &str = "JP_CFG_";

/// Convenience type for boxed errors.
type BoxedError = Box<dyn std::error::Error + Send + Sync>;

/// The global configuration for Jean Pierre.
#[derive(Debug, Clone, PartialEq, Config)]
#[config(rename_all = "snake_case")]
pub struct AppConfig {
    /// Inherit from a local ancestor or global configuration file.
    #[setting(optional)]
    pub inherit: bool,

    /// Directories to search for additional configuration files.
    ///
    /// Files in these directories can be loaded on demand using the `--cfg`
    /// flag. Use this to organize reusable configurations, such as personas or
    /// tool sets.
    ///
    /// For example, to load `.jp/agents/dev.toml`, add `.jp/agents` to this
    /// list and run `jp query --cfg dev`.
    #[setting(optional, merge = schematic::merge::append_vec, transform = util::vec_dedup)]
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
    pub extends: Vec<ExtendingRelativePath>,

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

impl PartialConfigDelta for PartialAppConfig {
    fn delta(&self, next: Self) -> Self {
        Self {
            // Any `extends` paths are interpreted at runtime, so we don't need to
            // store this information again, since the extended configuration is
            // already merged into the current one.
            extends: None,

            // Any `inherit` value is interpreted at runtime, so we don't need to
            // store this information again, since the config load logic will
            // already have stopped the merge process when it encounters an
            // `inherit` value of `true`.
            inherit: None,

            config_load_paths: delta_opt_vec(
                self.config_load_paths.as_ref(),
                next.config_load_paths,
            ),

            assistant: self.assistant.delta(next.assistant),
            conversation: self.conversation.delta(next.conversation),
            style: self.style.delta(next.style),
            editor: self.editor.delta(next.editor),
            template: self.template.delta(next.template),
            providers: self.providers.delta(next.providers),
        }
    }
}

impl ToPartial for AppConfig {
    fn to_partial(&self) -> Self::Partial {
        let defaults = Self::Partial::default();

        Self::Partial {
            inherit: partial_opt(&self.inherit, defaults.inherit),
            config_load_paths: partial_opt(&self.config_load_paths, defaults.config_load_paths),
            extends: partial_opt(&self.extends, defaults.extends),
            assistant: self.assistant.to_partial(),
            conversation: self.conversation.to_partial(),
            style: self.style.to_partial(),
            editor: self.editor.to_partial(),
            template: self.template.to_partial(),
            providers: self.providers.to_partial(),
        }
    }
}

impl AppConfig {
    /// Return a default configuration for testing purposes.
    ///
    /// This CANNOT be used in release mode.
    #[cfg(debug_assertions)]
    #[doc(hidden)]
    #[must_use]
    pub fn new_test() -> Self {
        use crate::{
            conversation::tool::RunMode,
            model::id::{Name, PartialModelIdConfig, ProviderId},
        };

        let mut partial = PartialAppConfig::empty();

        partial.conversation.title.generate.auto = Some(false);
        partial.conversation.tools.defaults.run = Some(RunMode::Ask);
        partial.assistant.model.id = PartialModelIdConfig {
            provider: Some(ProviderId::Anthropic),
            name: Some(Name("test".to_owned())),
        }
        .into();

        Self::from_partial(partial, vec![]).expect("valid config")
    }

    /// Build the schema for the configuration.
    ///
    /// Returns a [`Schema`] tree describing the structure of `AppConfig`, with
    /// [`SchemaType::Struct`] at each nested level containing a `fields` map of
    /// valid field names.
    #[must_use]
    pub fn schema() -> Schema {
        Self::build_schema(SchemaBuilder::default())
    }

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
        let mut output = Vec::new();
        let mut stack = vec![(Self::schema(), String::new())];

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

    /// Return a list of all environment variable names in the configuration.
    ///
    /// ```rust
    /// # use jp_config::AppConfig;
    ///
    /// assert_eq!(AppConfig::envs()[0..5], [
    ///     (
    ///         "config_load_paths".to_owned(),
    ///         "JP_CFG_CONFIG_LOAD_PATHS".to_owned()
    ///     ),
    ///     ("extends".to_owned(), "JP_CFG_EXTENDS".to_owned()),
    ///     ("inherit".to_owned(), "JP_CFG_INHERIT".to_owned()),
    ///     (
    ///         "template.values".to_owned(),
    ///         "JP_CFG_TEMPLATE_VALUES".to_owned()
    ///     ),
    ///     (
    ///         "style.typewriter.code_delay".to_owned(),
    ///         "JP_CFG_STYLE_TYPEWRITER_CODE_DELAY".to_owned()
    ///     ),
    /// ]);
    /// ```
    #[must_use]
    pub fn envs() -> IndexMap<String, String> {
        Self::fields()
            .into_iter()
            .map(|k| (format!("JP_CFG_{}", k.to_uppercase().replace('.', "_")), k))
            .map(|(k, v)| (v, k))
            .collect()
    }

    /// Convert this configuration to a partial configuration containing only
    /// values that differ from the default configuration.
    ///
    /// This is useful for serializing only the user-specified configuration
    /// values, excluding any defaults.
    #[must_use]
    pub fn to_partial(&self) -> PartialAppConfig {
        <Self as ToPartial>::to_partial(self)
    }

    /// Resolve all model ID aliases in the configuration.
    ///
    /// Converts every `ModelIdOrAliasConfig::Alias` to
    /// `ModelIdOrAliasConfig::Id` using `providers.llm.aliases`. After this
    /// call, no `Alias` variants remain in the configuration.
    ///
    /// This should be called once after `from_partial`, before the config
    /// is shared via `Arc`.
    ///
    /// # Errors
    ///
    /// Returns an error if an alias cannot be resolved.
    pub fn resolve_aliases(&mut self) -> Result<(), Error> {
        let aliases = &self.providers.llm.aliases;

        self.assistant
            .model
            .id
            .resolve_in_place(aliases)
            .map_err(|e| Error::Custom(format!("assistant.model.id: {e}").into()))?;

        if let Some(ref mut model) = self.conversation.inquiry.assistant.model {
            model.id.resolve_in_place(aliases).map_err(|e| {
                Error::Custom(format!("conversation.inquiry.assistant.model.id: {e}").into())
            })?;
        }

        if let Some(ref mut model) = self.conversation.title.generate.model {
            model.id.resolve_in_place(aliases).map_err(|e| {
                Error::Custom(format!("conversation.title.generate.model.id: {e}").into())
            })?;
        }

        Ok(())
    }
}

impl PartialAppConfig {
    /// Create a new empty partial configuration.
    #[must_use]
    pub fn empty() -> Self {
        <Self as PartialConfig>::empty()
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

    /// See [`PartialConfigDelta::delta`].
    #[must_use]
    pub fn delta(&self, next: Self) -> Self {
        <Self as PartialConfigDelta>::delta(self, next)
    }

    /// Resolve any model ID aliases in this partial config.
    ///
    /// Converts `PartialModelIdOrAliasConfig::Alias` variants to `Id` using
    /// the given alias map. Unresolvable aliases are left as-is (they'll
    /// produce errors when finalized into an `AppConfig`).
    ///
    /// Call this before storing a `PartialAppConfig` as a `ConfigDelta` in
    /// a conversation stream to maintain the invariant that stream configs
    /// contain only resolved model IDs.
    pub fn resolve_model_aliases(
        &mut self,
        aliases: &indexmap::IndexMap<String, model::id::ModelIdConfig>,
    ) {
        self.assistant.model.id.resolve_in_place(aliases);

        if let Some(ref mut model) = self.conversation.inquiry.assistant.model {
            model.id.resolve_in_place(aliases);
        }

        if let Some(ref mut model) = self.conversation.title.generate.model {
            model.id.resolve_in_place(aliases);
        }
    }

    /// Create a new partial configuration with stub values for testing
    /// purposes.
    ///
    /// # Panics
    ///
    /// This function cannot panic.
    #[doc(hidden)]
    #[must_use]
    pub fn stub() -> Self {
        use crate::{
            conversation::tool::RunMode,
            model::id::{PartialModelIdConfig, ProviderId},
        };

        let mut partial = Self::empty();
        partial.conversation.tools.defaults.run = Some(RunMode::Unattended);
        partial.assistant.model.id = PartialModelIdConfig {
            provider: Some(ProviderId::Ollama),
            name: Some("world".try_into().expect("valid name")),
        }
        .into();
        partial
    }
}

impl From<AppConfig> for PartialAppConfig {
    fn from(config: AppConfig) -> Self {
        config.to_partial()
    }
}

impl From<Arc<AppConfig>> for PartialAppConfig {
    fn from(config: Arc<AppConfig>) -> Self {
        config.to_partial()
    }
}

impl From<Arc<Self>> for AppConfig {
    fn from(config: Arc<Self>) -> Self {
        Arc::unwrap_or_clone(config)
    }
}

#[cfg(test)]
#[path = "lib_tests.rs"]
mod tests;
