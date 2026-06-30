//! Tool configuration for conversations.

use std::{fmt, str::FromStr};

use indexmap::IndexMap;
use schematic::{Config, ConfigEnum, ConfigError, HandlerError, PartialConfig as _};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use tracing::warn;

pub use crate::types::command::{
    CommandConfig, CommandConfigOrString, PartialCommandConfig, PartialCommandConfigOrString,
};
use crate::{
    assignment::{AssignKeyValue, AssignResult, KvAssignment, missing_key},
    assistant::PartialAssistantConfig,
    conversation::tool::{
        access::{AccessConfig, PartialAccessConfig},
        style::{DisplayStyleConfig, PartialDisplayStyleConfig},
    },
    delta::{PartialConfigDelta, delta_opt, delta_opt_partial, delta_opt_vec, delta_vec},
    fill::FillDefaults,
    partial::{ToPartial, partial_opt, partial_opt_config, partial_opts},
    types::json_value::JsonValue,
    util::merge_nested_indexmap,
    validate::Validator,
};

pub mod access;
pub mod style;

/// Tools configuration.
#[derive(Debug, Clone, PartialEq, Config)]
#[config(rename_all = "snake_case", allow_unknown_fields)]
pub struct ToolsConfig {
    /// Global config
    ///
    /// This section configures global defaults for all tools.
    #[setting(nested, rename = "*")]
    pub defaults: ToolsDefaultsConfig,

    /// Tool config
    ///
    /// This section configures individual tools.
    /// The key is the tool ID.
    #[setting(nested, flatten, merge = merge_nested_indexmap)]
    tools: IndexMap<String, ToolConfig>,
}

impl AssignKeyValue for PartialToolsConfig {
    fn assign(&mut self, mut kv: KvAssignment) -> AssignResult {
        match kv.key_string().as_str() {
            "" => kv.try_merge_object(self)?,
            _ if kv.p("*") => self.defaults.assign(kv)?,
            _ => kv.assign_to_entry(&mut self.tools)?,
        }

        Ok(())
    }
}

impl PartialConfigDelta for PartialToolsConfig {
    fn delta(&self, next: Self) -> Self {
        Self {
            defaults: self.defaults.delta(next.defaults),
            tools: next
                .tools
                .into_iter()
                .filter_map(|(name, next)| {
                    let next = match self.tools.get(&name) {
                        Some(prev) if prev == &next => return None,
                        Some(prev) => prev.delta(next),
                        None => next,
                    };

                    Some((name, next))
                })
                .collect(),
        }
    }
}

impl FillDefaults for PartialToolsConfig {
    fn fill_from(self, defaults: Self) -> Self {
        Self {
            defaults: self.defaults.fill_from(defaults.defaults),
            tools: self.tools,
        }
    }
}

impl ToPartial for ToolsConfig {
    fn to_partial(&self) -> Self::Partial {
        Self::Partial {
            defaults: self.defaults.to_partial(),
            tools: self
                .tools
                .iter()
                .map(|(k, v)| (k.clone(), v.to_partial()))
                .collect(),
        }
    }
}

impl ToolsConfig {
    /// Get a tool configuration by name.
    ///
    /// This returns [`ToolConfigWithDefaults`], merging the global defaults
    /// into the tool configuration.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<ToolConfigWithDefaults> {
        self.tools
            .get(name)
            .cloned()
            .map(|tool| ToolConfigWithDefaults {
                tool,
                defaults: self.defaults.clone(),
            })
    }

    /// Returns `true` if a tool with the given name is configured.
    #[must_use]
    pub fn contains(&self, name: &str) -> bool {
        self.tools.contains_key(name)
    }

    /// Iterate tool configurations.
    ///
    /// This returns `(&str, [ToolConfigWithDefaults])`, merging the global
    /// defaults into the tool configurations.
    pub fn iter(&self) -> impl Iterator<Item = (&str, ToolConfigWithDefaults)> {
        self.tools.iter().map(|(k, v)| {
            (k.as_str(), ToolConfigWithDefaults {
                tool: v.clone(),
                defaults: self.defaults.clone(),
            })
        })
    }

    /// Iterate tool configurations mutably.
    pub fn iter_mut(&mut self) -> impl Iterator<Item = (&str, &mut ToolConfig)> {
        self.tools.iter_mut().map(|(k, v)| (k.as_str(), v))
    }

    /// Insert a tool configuration.
    pub fn insert(&mut self, name: String, tool: ToolConfig) {
        self.tools.insert(name, tool);
    }
}

impl Validator for ToolsConfig {
    /// Validate cross-field invariants on the tools configuration.
    fn validate(&self) -> Result<(), ConfigError> {
        reject_access_on_non_local_tools(self)
    }
}

/// Reject `access` on tools whose finalized source is `builtin` or `mcp`.
///
/// `access` is the local-subprocess contract: it is serialized into the
/// `Context` that local tool binaries self-check.
/// Builtin tools run in-process and MCP tools run on external servers, so
/// neither consumes `access` — accepting it there would create false
/// confidence in a security-relevant field.
fn reject_access_on_non_local_tools(tools: &ToolsConfig) -> Result<(), ConfigError> {
    for (name, tool) in tools.iter() {
        if tool.access().is_none() {
            continue;
        }
        let kind = match tool.source() {
            ToolSource::Local { .. } => continue,
            ToolSource::Builtin { .. } => "builtin",
            ToolSource::Mcp { .. } => "mcp",
        };
        return Err(HandlerError::new(format!(
            "conversation.tools.{name}: `access` is only supported on local tools, but '{name}' \
             is a {kind} tool"
        ))
        .into());
    }
    Ok(())
}

/// Tools defaults configuration.
#[derive(Debug, Clone, PartialEq, Config)]
#[config(rename_all = "snake_case")]
pub struct ToolsDefaultsConfig {
    /// Default tool enablement, and which directives may change it.
    ///
    /// Applies to every tool that doesn't set its own `enable`.
    /// Accepts a bool (`true`/`false`), a legacy string (`"on"`, `"off"`,
    /// `"explicit"`, `"always"`), or a `{ state, allow_toggle }` table.
    /// When unset, tools fall back to enabled and freely toggleable.
    #[setting(nested)]
    pub enable: Option<EnableConfig>,

    /// How to run the tool.
    ///
    /// - `ask`: Ask for confirmation before running the tool.
    /// - `unattended`: Run the tool without asking for confirmation.
    /// - `edit`: Open an editor to edit the tool call before running it.
    /// - `skip`: Skip running the tool.
    #[setting(required)]
    pub run: RunMode,

    /// When to run a tool's custom argument formatter relative to the approval
    /// prompt.
    ///
    /// Only affects [`style::ParametersStyle::Custom`] (a user-configured shell
    /// command).
    /// Built-in parameter styles (`json`, `function_call`, `off`) are pure
    /// transformations and always render before the prompt regardless of this
    /// field.
    ///
    /// - `ask`: Defer the custom formatter until after approval (safe default
    ///   — keeps an untrusted shell command from running unprompted).
    /// - `unattended`: Run the custom formatter ahead of the approval prompt so
    ///   the user sees the rendered call before deciding.
    ///
    /// If unset, derives from [`run`]: `Ask`/`Edit`/`Skip` map to `Ask`;
    /// `Unattended` maps to `Unattended`.
    ///
    /// [`run`]: Self::run
    pub format: Option<FormatMode>,

    /// How to deliver the results of the tool to the assistant.
    ///
    /// - `unattended`: Always deliver the results of the tool call.
    /// - `ask`: Ask for confirmation before delivering the results.
    /// - `edit`: Open an editor to edit the result before delivering it.
    /// - `skip`: Skip delivering the results.
    #[setting(default)]
    pub result: ResultMode,

    /// How to display the results of the tool in the terminal.
    #[setting(nested)]
    pub style: DisplayStyleConfig,
}

impl AssignKeyValue for PartialToolsDefaultsConfig {
    fn assign(&mut self, mut kv: KvAssignment) -> AssignResult {
        match kv.key_string().as_str() {
            "enable" => self.enable = kv.try_some_object_bool_or_from_str()?,
            _ if kv.p("enable") => self.enable.assign(kv)?,
            "run" => self.run = kv.try_some_from_str()?,
            "format" => self.format = kv.try_some_from_str()?,
            "result" => self.result = kv.try_some_from_str()?,
            _ if kv.p("style") => self.style.assign(kv)?,
            _ => return missing_key(&kv),
        }

        Ok(())
    }
}

impl PartialConfigDelta for PartialToolsDefaultsConfig {
    fn delta(&self, next: Self) -> Self {
        Self {
            enable: delta_opt_partial(self.enable.as_ref(), next.enable),
            run: delta_opt(self.run.as_ref(), next.run),
            format: delta_opt(self.format.as_ref(), next.format),
            result: delta_opt(self.result.as_ref(), next.result),
            style: self.style.delta(next.style),
        }
    }
}

impl FillDefaults for PartialToolsDefaultsConfig {
    fn fill_from(self, defaults: Self) -> Self {
        Self {
            enable: self.enable.or(defaults.enable),
            run: self.run.or(defaults.run),
            format: self.format.or(defaults.format),
            result: self.result.or(defaults.result),
            style: self.style.fill_from(defaults.style),
        }
    }
}

impl ToPartial for ToolsDefaultsConfig {
    fn to_partial(&self) -> Self::Partial {
        let defaults = Self::Partial::default();

        Self::Partial {
            enable: partial_opt_config(self.enable.as_ref(), defaults.enable),
            run: partial_opt(&self.run, defaults.run),
            format: partial_opts(self.format.as_ref(), defaults.format),
            result: partial_opt(&self.result, defaults.result),
            style: self.style.to_partial(),
        }
    }
}

/// Tool configuration.
#[derive(Debug, Clone, PartialEq, Config)]
#[config(rename_all = "snake_case")]
pub struct ToolConfig {
    /// The source of the tool.
    ///
    /// - `builtin`: Use a built-in tool.
    /// - `local`: Use a locally defined tool (shell command).
    /// - `mcp`: Use a tool from an MCP server.
    #[setting(required)]
    pub source: ToolSource,

    /// Whether the tool is enabled, and which directives may change that.
    ///
    /// Accepts a bool (`true`/`false`), a legacy string (`"on"`, `"off"`,
    /// `"explicit"`, `"always"`), or a `{ state, allow_toggle }` table.
    /// When unset, inherits from `[conversation.tools.'*']`, then falls back to
    /// enabled and freely toggleable.
    #[setting(nested)]
    pub enable: Option<EnableConfig>,

    /// The command to run.
    /// Only used for local tools.
    ///
    /// Can be a simple string (e.g. `ls -la`) or a structured object with
    /// `program`, `args`, and `shell` properties.
    #[setting(nested)]
    pub command: Option<CommandConfigOrString>,

    /// A short summary of what the tool does.
    ///
    /// This is always included in the tool definition sent to the LLM.
    /// It should be concise enough to give the LLM a general idea of the tool's
    /// purpose without consuming excessive context.
    ///
    /// If not set, falls back to [`description`].
    ///
    /// [`description`]: Self::description
    pub summary: Option<String>,

    /// The full description of the tool.
    ///
    /// This provides detailed information about the tool's behavior, arguments,
    /// and edge cases.
    /// It is NOT sent to the LLM by default, instead, it is made available on
    /// demand via the `describe_tools` built-in tool.
    ///
    /// This will override any existing description, such as the one from an MCP
    /// server, or a built-in tool.
    pub description: Option<String>,

    /// Usage examples for the tool.
    ///
    /// Like `description`, examples are loaded on demand via `describe_tools`
    /// rather than being sent in every request.
    pub examples: Option<String>,

    /// The parameters expected by the tool.
    ///
    /// For `local` tools, omitting this will result in a tool that takes no
    /// parameters.
    /// For `mcp` or `builtin` tools, omitting this keeps the original
    /// parameters from the tool definition, but you can override existing
    /// parameters by specifying them here.
    ///
    /// Overriding parameters is allowed in narrow cases, such as flipping an
    /// argument from optional to required, defining an enumeration of allowed
    /// values, or forcing a specific value by setting a single enum value.
    /// You CANNOT change the type of the argument, its name, or any other
    /// properties that would break the tool's original argument expectations.
    #[setting(nested, merge = merge_nested_indexmap)]
    pub parameters: IndexMap<String, ToolParameterConfig>,

    /// How to run the tool.
    ///
    /// Overrides the global default.
    pub run: Option<RunMode>,

    /// When to run the tool's custom argument formatter relative to the
    /// approval prompt.
    ///
    /// Only affects [`style::ParametersStyle::Custom`]; see
    /// [`ToolsDefaultsConfig::format`] for details.
    ///
    /// Overrides the global default.
    /// If unset, derives from [`run`]: `Ask`/`Edit`/`Skip` map to
    /// `FormatMode::Ask`; `Unattended` maps to `FormatMode::Unattended`.
    ///
    /// [`run`]: Self::run
    pub format: Option<FormatMode>,

    /// How to deliver the results of the tool to the assistant.
    ///
    /// Overrides the global default.
    pub result: Option<ResultMode>,

    /// How to display the results of the tool in the terminal.
    ///
    /// Overrides the global default.
    /// The error overlay lives at `style.error.*` (see
    /// [`DisplayStyleConfig::error`]).
    #[setting(nested)]
    pub style: Option<DisplayStyleConfig>,

    /// Configuration for questions that the tool may ask during execution.
    ///
    /// Question IDs are defined by the tool implementation and should be
    /// documented by the tool.
    /// For example, `fs_create_file` uses `overwrite_file` when a file already
    /// exists.
    #[setting(nested, merge = merge_nested_indexmap)]
    pub questions: IndexMap<String, QuestionConfig>,

    /// Per-tool options passed to the tool at runtime.
    ///
    /// A free-form map of key-value pairs that configure tool behavior.
    /// Each tool defines its own supported options and defaults.
    /// Unknown options are silently forwarded.
    #[setting(nested, merge = merge_nested_indexmap)]
    pub options: IndexMap<String, JsonValue>,

    /// Filesystem access grants for the tool.
    ///
    /// Declares which workspace-relative paths the tool may touch and what it
    /// may do there.
    /// When absent, the tool keeps unrestricted (but workspace-confined)
    /// access; declaring any rule switches the tool to default-deny.
    #[setting(nested)]
    pub access: Option<AccessConfig>,
}

impl AssignKeyValue for PartialToolConfig {
    fn assign(&mut self, mut kv: KvAssignment) -> AssignResult {
        match kv.key_string().as_str() {
            "" => kv.try_merge_object(self)?,
            "source" => self.source = kv.try_some_from_str()?,
            "enable" => self.enable = kv.try_some_object_bool_or_from_str()?,
            _ if kv.p("enable") => self.enable.assign(kv)?,
            _ if kv.p("command") => self.command.assign(kv)?,
            "summary" => self.summary = kv.try_some_string()?,
            "description" => self.description = kv.try_some_string()?,
            "examples" => self.examples = kv.try_some_string()?,
            "parameters" => self.parameters = kv.try_object()?,
            "run" => self.run = kv.try_some_from_str()?,
            "format" => self.format = kv.try_some_from_str()?,
            "result" => self.result = kv.try_some_from_str()?,
            _ if kv.p("style") => self.style.assign(kv)?,
            "questions" => self.questions = kv.try_object()?,
            _ if kv.p("options") => kv.assign_to_entry(&mut self.options)?,
            _ if kv.p("access") => self.access.assign(kv)?,
            _ => return missing_key(&kv),
        }

        Ok(())
    }
}

impl PartialConfigDelta for PartialToolConfig {
    fn delta(&self, next: Self) -> Self {
        Self {
            source: delta_opt(self.source.as_ref(), next.source),
            enable: delta_opt_partial(self.enable.as_ref(), next.enable),
            command: delta_opt_partial(self.command.as_ref(), next.command),
            summary: delta_opt(self.summary.as_ref(), next.summary),
            description: delta_opt(self.description.as_ref(), next.description),
            examples: delta_opt(self.examples.as_ref(), next.examples),
            parameters: next
                .parameters
                .into_iter()
                .filter_map(|(k, next)| {
                    let prev = self.parameters.get(&k);
                    if prev.is_some_and(|prev| prev == &next) {
                        return None;
                    }

                    let next = match prev {
                        Some(prev) => prev.delta(next),
                        None => next,
                    };

                    Some((k, next))
                })
                .collect(),
            run: delta_opt(self.run.as_ref(), next.run),
            format: delta_opt(self.format.as_ref(), next.format),
            result: delta_opt(self.result.as_ref(), next.result),
            style: delta_opt_partial(self.style.as_ref(), next.style),
            questions: next
                .questions
                .into_iter()
                .filter_map(|(k, next)| {
                    let prev = self.questions.get(&k);
                    if prev.is_some_and(|prev| prev == &next) {
                        return None;
                    }

                    let next = match prev {
                        Some(prev) => prev.delta(next),
                        None => next,
                    };

                    Some((k, next))
                })
                .collect(),
            options: next
                .options
                .into_iter()
                .filter_map(|(name, next)| {
                    if self.options.get(&name).is_some_and(|prev| prev == &next) {
                        return None;
                    }
                    Some((name, next))
                })
                .collect(),
            access: delta_opt_partial(self.access.as_ref(), next.access),
        }
    }
}

impl ToPartial for ToolConfig {
    fn to_partial(&self) -> Self::Partial {
        let defaults = Self::Partial::default();

        Self::Partial {
            source: partial_opt(&self.source, defaults.source),
            enable: partial_opt_config(self.enable.as_ref(), defaults.enable),
            command: partial_opt_config(self.command.as_ref(), defaults.command),
            summary: partial_opts(self.summary.as_ref(), defaults.summary),
            description: partial_opts(self.description.as_ref(), defaults.description),
            examples: partial_opts(self.examples.as_ref(), defaults.examples),
            parameters: self
                .parameters
                .iter()
                .map(|(k, v)| (k.clone(), v.to_partial()))
                .collect(),
            run: partial_opts(self.run.as_ref(), defaults.run),
            format: partial_opts(self.format.as_ref(), defaults.format),
            result: partial_opts(self.result.as_ref(), defaults.result),
            style: partial_opt_config(self.style.as_ref(), defaults.style),
            questions: self
                .questions
                .iter()
                .map(|(k, v)| (k.clone(), v.to_partial()))
                .collect(),
            options: self
                .options
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
            access: partial_opt_config(self.access.as_ref(), defaults.access),
        }
    }
}

/// Tool parameter configuration.
///
/// This type doubles as a recursive JSON Schema node: `items` points to the
/// schema for array elements, `properties` describes the fields of object
/// elements.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Config)]
#[config(rename_all = "snake_case")]
pub struct ToolParameterConfig {
    /// The type of the parameter.
    #[setting(nested)]
    #[serde(rename = "type")]
    pub kind: OneOrManyTypes,

    /// The default value of the parameter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<Value>,

    /// Whether the parameter is required.
    #[setting(default)]
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub required: bool,

    /// A short summary of the parameter.
    ///
    /// This is included in the JSON schema sent to the LLM.
    /// If not set, falls back to [`description`].
    ///
    /// [`description`]: Self::description
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,

    /// The full description of the parameter.
    ///
    /// Detailed usage information loaded on demand via `describe_tools`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Usage examples for the parameter.
    ///
    /// Loaded on demand via `describe_tools`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub examples: Option<String>,

    /// A list of possible values for the parameter.
    #[serde(default, rename = "enum", skip_serializing_if = "Vec::is_empty")]
    pub enumeration: Vec<Value>,

    /// Schema for array items (recursive).
    #[setting(nested)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[expect(clippy::use_self, reason = "macro can't resolve `Self`")]
    pub items: Option<Box<ToolParameterConfig>>,

    /// Sub-properties for object-typed parameters (recursive).
    #[setting(nested, merge = merge_nested_indexmap)]
    #[serde(default, skip_serializing_if = "IndexMap::is_empty")]
    #[expect(clippy::use_self, reason = "macro can't resolve `Self`")]
    pub properties: IndexMap<String, ToolParameterConfig>,
}

impl PartialConfigDelta for PartialToolParameterConfig {
    fn delta(&self, next: Self) -> Self {
        Self {
            kind: self.kind.delta(next.kind),
            default: delta_opt(self.default.as_ref(), next.default),
            required: delta_opt(self.required.as_ref(), next.required),
            summary: delta_opt(self.summary.as_ref(), next.summary),
            description: delta_opt(self.description.as_ref(), next.description),
            examples: delta_opt(self.examples.as_ref(), next.examples),
            enumeration: delta_opt_vec(self.enumeration.as_ref(), next.enumeration),
            items: delta_opt(self.items.as_ref(), next.items),
            properties: next
                .properties
                .into_iter()
                .filter_map(|(k, next)| {
                    let prev = self.properties.get(&k);
                    if prev.is_some_and(|prev| prev == &next) {
                        return None;
                    }
                    let next = match prev {
                        Some(prev) => prev.delta(next),
                        None => next,
                    };
                    Some((k, next))
                })
                .collect(),
        }
    }
}

impl ToPartial for ToolParameterConfig {
    fn to_partial(&self) -> Self::Partial {
        let defaults = Self::Partial::default();

        Self::Partial {
            kind: self.kind.to_partial(),
            default: partial_opts(self.default.as_ref(), defaults.default),
            required: partial_opt(&self.required, defaults.required),
            summary: partial_opts(self.summary.as_ref(), defaults.summary),
            description: partial_opts(self.description.as_ref(), defaults.description),
            examples: partial_opts(self.examples.as_ref(), defaults.examples),
            enumeration: if self.enumeration.is_empty() {
                None
            } else {
                Some(self.enumeration.clone())
            },
            items: self.items.as_ref().map(|v| Box::new(v.to_partial())),
            properties: self
                .properties
                .iter()
                .map(|(k, v)| (k.clone(), v.to_partial()))
                .collect(),
        }
    }
}

/// A type that can be either a single type or a list of types.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Config)]
#[serde(untagged)]
pub enum OneOrManyTypes {
    /// A single type.
    #[setting(empty)]
    One(String),

    /// A list of types.
    Many(Vec<String>),
}

impl PartialConfigDelta for PartialOneOrManyTypes {
    fn delta(&self, next: Self) -> Self {
        match (self, next) {
            (Self::Many(prev), Self::Many(next)) => Self::Many(delta_vec(prev, next)),
            (_, next) => next,
        }
    }
}

impl ToPartial for OneOrManyTypes {
    fn to_partial(&self) -> Self::Partial {
        match self {
            Self::One(v) => Self::Partial::One(v.to_owned()),
            Self::Many(v) => Self::Partial::Many(v.to_owned()),
        }
    }
}

impl From<String> for OneOrManyTypes {
    fn from(s: String) -> Self {
        Self::One(s)
    }
}

impl From<Vec<String>> for OneOrManyTypes {
    fn from(v: Vec<String>) -> Self {
        Self::Many(v)
    }
}

impl OneOrManyTypes {
    /// Return whether the type can be the given type.
    #[must_use]
    pub fn has_type(&self, type_: &str) -> bool {
        match self {
            Self::One(v) => v.as_str() == type_,
            Self::Many(v) => v.iter().any(|v| v == type_),
        }
    }

    /// Return whether the type is exactly the given type.
    #[must_use]
    pub fn is_type(&self, type_: &str) -> bool {
        match self {
            Self::One(v) => v.as_str() == type_,
            Self::Many(v) => v.len() == 1 && v[0] == type_,
        }
    }
}

impl ToolParameterConfig {
    /// Return whether the parameter is required.
    #[must_use]
    pub const fn is_required(&self) -> bool {
        self.required
    }

    /// Convert the parameter to a JSON schema.
    pub fn to_json_schema(&self) -> Value {
        let mut map = Map::new();
        map.insert("type".to_owned(), match &self.kind {
            OneOrManyTypes::One(v) => v.clone().into(),
            OneOrManyTypes::Many(v) => v.clone().into(),
        });

        if let Some(desc) = self.summary.as_deref().or(self.description.as_deref()) {
            map.insert("description".to_owned(), desc.into());
        }

        if let Some(default) = self.default.clone() {
            map.insert("default".to_owned(), default);
        }

        if !self.enumeration.is_empty() {
            map.insert("enum".to_owned(), self.enumeration.as_slice().into());
        }

        if let Some(items) = self.items.as_ref() {
            if !self.kind.is_type("array") {
                warn!("Unexpected `items` property for non-array type");
            }
            map.insert("items".to_owned(), items.to_json_schema());
        }

        if !self.properties.is_empty() {
            let props: Map<_, _> = self
                .properties
                .iter()
                .map(|(k, v)| (k.clone(), v.to_json_schema()))
                .collect();
            map.insert("properties".to_owned(), Value::Object(props));

            let required: Vec<_> = self
                .properties
                .iter()
                .filter(|(_, v)| v.required)
                .map(|(k, _)| Value::String(k.clone()))
                .collect();
            if !required.is_empty() {
                map.insert("required".to_owned(), Value::Array(required));
            }
        }

        Value::Object(map)
    }
}

/// The source of a tool.
#[derive(Debug, Clone, PartialEq)]
pub enum ToolSource {
    /// Use a built-in tool.
    Builtin {
        /// The name of the tool to use.
        ///
        /// If not specified, it is inferred from the key in the
        /// [`super::ConversationConfig::tools`] map.
        tool: Option<String>,
    },

    /// Use a locally defined tool.
    Local {
        /// The name of the tool to use.
        ///
        /// If not specified, it is inferred from the key in the
        /// [`super::ConversationConfig::tools`] map.
        // TODO: What's the reason for specifying this for local tools? It seems
        // to me that there is only one way to define the tool name, in
        // `ToolsConfig`, so it should be inferred from the key? For `mcp` tools
        // it makes sense, if you want to rename the tool from the server's
        // original name.
        tool: Option<String>,
    },

    /// Use a tool from a MCP server.
    Mcp {
        /// The name of the MCP server that contains the tool.
        ///
        /// The server name is required — the source string must be of the form
        /// `mcp.<server>` or `mcp.<server>.<tool>`.
        /// The legacy shapes `mcp` and `mcp..<tool>` are rejected because the
        /// implicit cross-server lookup they enabled was both order-dependent
        /// (whichever server happened to be iterated first) and incompatible
        /// with optional MCP servers (a failed candidate would abort the whole
        /// resolution).
        server: String,

        /// The name of the tool to use.
        ///
        /// If not specified, it is inferred from the key in the
        /// [`super::ConversationConfig::tools`] map.
        tool: Option<String>,
    },
}

impl<'de> Deserialize<'de> for ToolSource {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        String::deserialize(deserializer)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

impl Serialize for ToolSource {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        let s = match self {
            Self::Builtin { tool } => tool
                .as_ref()
                .map_or_else(|| "builtin".to_string(), |tool| format!("builtin.{tool}")),
            Self::Local { tool } => tool
                .as_ref()
                .map_or_else(|| "local".to_string(), |tool| format!("local.{tool}")),
            Self::Mcp { server, tool } => {
                let mut s = format!("mcp.{server}");
                if let Some(tool) = tool {
                    s.push('.');
                    s.push_str(tool);
                }
                s
            }
        };
        serializer.serialize_str(&s)
    }
}

impl FromStr for ToolSource {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (source, tool) = s
            .split_once('.')
            .map(|(a, b)| (a, Some(b.to_owned())))
            .unwrap_or((s, None));
        match source {
            "builtin" => Ok(Self::Builtin { tool }),
            "local" => Ok(Self::Local { tool }),
            "mcp" => {
                let Some(rest) = tool else {
                    return Err("MCP tool source must name a server: use `mcp.<server>` or \
                                `mcp.<server>.<tool>` (legacy `mcp` is no longer accepted)."
                        .to_owned());
                };

                let (server, tool) = match rest.split_once('.') {
                    Some((server, tool)) => (server.to_owned(), Some(tool.to_owned())),
                    None => (rest, None),
                };

                if server.is_empty() {
                    return Err("MCP tool source must name a server: use `mcp.<server>` or \
                                `mcp.<server>.<tool>` (legacy `mcp..<tool>` is no longer \
                                accepted)."
                        .to_owned());
                }

                Ok(Self::Mcp { server, tool })
            }
            _ => Err(format!(
                "Unknown tool source: {source}, must be one of: builtin, local, mcp"
            )),
        }
    }
}

impl ToolSource {
    /// Return whether the tool is from an MCP server.
    #[must_use]
    pub const fn is_mcp(&self) -> bool {
        matches!(self, Self::Mcp { .. })
    }

    /// Return the custom name of the tool, if any.
    #[must_use]
    pub fn tool_name(&self) -> Option<&str> {
        match self {
            Self::Builtin { tool } | Self::Local { tool } | Self::Mcp { tool, .. } => {
                tool.as_deref()
            }
        }
    }
}

impl schematic::Schematic for ToolSource {
    fn schema_name() -> Option<String> {
        Some("tool_source".to_owned())
    }

    fn build_schema(mut schema: schematic::SchemaBuilder) -> schematic::Schema {
        schema.string(schematic::schema::StringType::default())
    }
}

/// The run mode of a tool.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize, ConfigEnum)]
#[serde(rename_all = "lowercase")]
pub enum RunMode {
    /// Ask for confirmation before running the tool.
    #[default]
    Ask,

    /// Run the tool without asking for confirmation.
    Unattended,

    /// Open an editor to edit the tool call before running it.
    Edit,

    /// Skip running the tool.
    Skip,
}

/// When to run a tool's argument formatter (custom-style parameter render).
///
/// This controls whether the formatter — which may make read-only network
/// calls or other side-effect-free I/O to render the rendered tool call — runs
/// *before* the user is asked for permission, or *after*.
///
/// `Ask` defers rendering until after approval (safe default for untrusted
/// tools).
/// `Unattended` runs the formatter up front, so the rendered output appears in
/// the approval prompt — the user makes their decision based on the rendered
/// call, not raw arguments.
///
/// **Contract**: Tools that opt into `Unattended` MUST be side-effect-free in
/// format mode.
/// They MAY make read-only network calls.
/// They MUST NOT mutate any state, write files, or send notifications.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize, ConfigEnum)]
#[serde(rename_all = "lowercase")]
pub enum FormatMode {
    /// Defer formatter execution until after approval.
    ///
    /// Safe default: an untrusted tool's formatter cannot run before the user
    /// has explicitly approved running the tool.
    #[default]
    Ask,

    /// Run the formatter ahead of the approval prompt.
    ///
    /// Only set this for trusted, side-effect-free formatters.
    /// The user sees the rendered tool call before deciding whether to approve.
    Unattended,
}

/// How to deliver the results of the tool to the assistant.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize, ConfigEnum)]
#[serde(rename_all = "lowercase")]
pub enum ResultMode {
    /// Always deliver the results of the tool call.
    #[default]
    Unattended,

    /// Ask for confirmation before delivering the results of the tool call.
    Ask,

    /// Open an editor to edit the tool call result before delivering it.
    Edit,

    /// Skip delivering the results of the tool call.
    Skip,
}

/// Tool configuration with global defaults.
#[derive(Debug, Clone)]
pub struct ToolConfigWithDefaults {
    /// The tool configuration.
    tool: ToolConfig,

    /// The global defaults.
    defaults: ToolsDefaultsConfig,
}

impl ToolConfigWithDefaults {
    /// Return the resolved [`Enable`] for this tool.
    ///
    /// Fills each field from the per-tool config, then the global defaults,
    /// then the hardcoded fallback (`state = true`, `allow_toggle = any`).
    ///
    /// A per-tool `#[setting(default = true)]` would not work here: the schema
    /// default is keyed by field, not by map entry, so an entry that omits
    /// `enable` has no per-entry default to fall back on.
    /// The fallback is applied at resolution time instead.
    #[must_use]
    pub fn effective_enable(&self) -> Enable {
        let tool = self
            .tool
            .enable
            .as_ref()
            .map(ToPartial::to_partial)
            .unwrap_or_default();
        let defaults = self
            .defaults
            .enable
            .as_ref()
            .map(ToPartial::to_partial)
            .unwrap_or_default();
        tool.effective(&defaults)
    }

    /// Return whether the tool is effectively enabled.
    ///
    /// If neither the tool nor the global defaults set a value, the tool is
    /// enabled.
    #[must_use]
    pub fn is_enabled(&self) -> bool {
        self.effective_enable().is_enabled()
    }

    /// Return the command to run the tool.
    #[must_use]
    pub fn command(&self) -> Option<CommandConfig> {
        self.tool
            .command
            .clone()
            .map(CommandConfigOrString::command)
    }

    /// Return the source of the tool.
    #[must_use]
    pub const fn source(&self) -> &ToolSource {
        &self.tool.source
    }

    /// Return the summary of the tool.
    ///
    /// Falls back to [`description`] if no summary is set.
    ///
    /// [`description`]: Self::description
    #[must_use]
    pub fn summary(&self) -> Option<&str> {
        self.tool
            .summary
            .as_deref()
            .or(self.tool.description.as_deref())
    }

    /// Return the full description of the tool.
    #[must_use]
    pub fn description(&self) -> Option<&str> {
        self.tool.description.as_deref()
    }

    /// Return the usage examples of the tool.
    #[must_use]
    pub fn examples(&self) -> Option<&str> {
        self.tool.examples.as_deref()
    }

    /// Return the parameters of the tool.
    #[must_use]
    pub const fn parameters(&self) -> &IndexMap<String, ToolParameterConfig> {
        &self.tool.parameters
    }

    /// Return the run mode of the tool.
    #[must_use]
    pub fn run(&self) -> RunMode {
        self.tool.run.unwrap_or(self.defaults.run)
    }

    /// Return a mutable reference to the run mode of the tool.
    #[must_use]
    pub fn run_mut(&mut self) -> &mut RunMode {
        self.tool.run.get_or_insert(self.defaults.run)
    }

    /// Return the format mode of the tool.
    ///
    /// Only affects [`style::ParametersStyle::Custom`] (a user-configured shell
    /// command).
    /// Built-in parameter styles (`json`, `function_call`, `off`) are pure
    /// transformations and always render before the approval prompt regardless
    /// of this value — see [`ToolsDefaultsConfig::format`] for the full
    /// contract.
    ///
    /// If neither the tool nor the global defaults set a `format` value, the
    /// mode is derived from [`run`]: `Ask`/`Edit`/`Skip` map to
    /// `FormatMode::Ask` (custom formatter runs after approval, safe default
    /// for an untrusted shell command; `Skip` is grouped here so we don't run a
    /// formatter for a tool that's about to be discarded); `Unattended` maps to
    /// `FormatMode::Unattended` (the tool was already going to run without an
    /// approval prompt anyway).
    ///
    /// [`run`]: Self::run
    #[must_use]
    pub fn format(&self) -> FormatMode {
        self.tool
            .format
            .or(self.defaults.format)
            .unwrap_or_else(|| match self.run() {
                // `Skip` would run the formatter and then immediately
                // discard the tool, leaving an orphan preview on screen.
                // Default to `Ask` so the formatter only runs when the
                // tool is actually going to be invoked.
                RunMode::Ask | RunMode::Edit | RunMode::Skip => FormatMode::Ask,
                RunMode::Unattended => FormatMode::Unattended,
            })
    }

    /// Return the result mode of the tool.
    #[must_use]
    pub fn result(&self) -> ResultMode {
        self.tool.result.unwrap_or(self.defaults.result)
    }

    /// Return a mutable reference to the result mode of the tool.
    #[must_use]
    pub fn result_mut(&mut self) -> &mut ResultMode {
        self.tool.result.get_or_insert(self.defaults.result)
    }

    /// Return the display style of the tool.
    #[must_use]
    pub fn style(&self) -> &DisplayStyleConfig {
        self.tool.style.as_ref().unwrap_or(&self.defaults.style)
    }

    /// Return the questions configuration of the tool.
    #[must_use]
    pub const fn questions(&self) -> &IndexMap<String, QuestionConfig> {
        &self.tool.questions
    }

    /// Return the per-tool options map.
    #[must_use]
    pub const fn options(&self) -> &IndexMap<String, JsonValue> {
        &self.tool.options
    }

    /// Return the filesystem access grants for the tool, if declared.
    #[must_use]
    pub const fn access(&self) -> Option<&AccessConfig> {
        self.tool.access.as_ref()
    }

    /// Return the question target for the given question ID.
    #[must_use]
    pub fn question_target(&self, question_id: &str) -> Option<&QuestionTarget> {
        self.tool.questions.get(question_id).map(|q| &q.target)
    }

    /// Get an automated answer for a question.
    ///
    /// Returns the configured answer if one exists for the given question ID,
    /// otherwise returns `None`.
    #[must_use]
    pub fn get_answer(&self, question_id: &str) -> Option<&Value> {
        self.tool
            .questions
            .get(question_id)
            .and_then(|q| q.answer.as_ref())
    }
}

/// Question configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Config)]
#[config(rename_all = "snake_case")]
pub struct QuestionConfig {
    /// The target of the question.
    ///
    /// This determines whether the question is asked interactively to the user,
    /// or sent to the assistant to be answered.
    pub target: QuestionTarget,

    /// The fixed answer to the question.
    ///
    /// If this is set, the question will not be presented to the target, but
    /// will always be answered with the given value.
    // TODO: We should add an enumeration of possible options:
    //
    // - Fixed answer
    // - Prompt once per turn
    // - Prompt once per conversation
    pub answer: Option<Value>,
}

impl PartialConfigDelta for PartialQuestionConfig {
    fn delta(&self, next: Self) -> Self {
        Self {
            target: delta_opt(self.target.as_ref(), next.target),
            answer: delta_opt(self.answer.as_ref(), next.answer),
        }
    }
}

impl ToPartial for QuestionConfig {
    fn to_partial(&self) -> Self::Partial {
        let defaults = Self::Partial::default();

        Self::Partial {
            target: partial_opt(&self.target, defaults.target),
            answer: partial_opts(self.answer.as_ref(), defaults.answer),
        }
    }
}

/// The target of a question.
///
/// Accepts a string (`"user"` or `"assistant"`) or a map with assistant config
/// overrides for the inquiry model.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum QuestionTarget {
    /// Ask the question to the user.
    #[default]
    User,

    /// Ask the question to the assistant.
    ///
    /// The partial config overrides the global inquiry config, which in turn
    /// overrides the parent assistant config.
    /// An empty partial (all `None`) means "use global inquiry defaults."
    Assistant(Box<PartialAssistantConfig>),
}

impl QuestionTarget {
    /// Returns `true` if the target is the user.
    #[must_use]
    pub const fn is_user(&self) -> bool {
        matches!(self, Self::User)
    }

    /// Returns `true` if the target is the assistant.
    #[must_use]
    pub const fn is_assistant(&self) -> bool {
        matches!(self, Self::Assistant(_))
    }
}

impl Serialize for QuestionTarget {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            Self::User => serializer.serialize_str("user"),
            Self::Assistant(config) if config.is_empty() => serializer.serialize_str("assistant"),
            Self::Assistant(config) => config.serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for QuestionTarget {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct QuestionTargetVisitor;

        impl<'de> serde::de::Visitor<'de> for QuestionTargetVisitor {
            type Value = QuestionTarget;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter
                    .write_str("\"user\", \"assistant\", or a map of assistant config overrides")
            }

            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<QuestionTarget, E> {
                match v {
                    "user" => Ok(QuestionTarget::User),
                    "assistant" => Ok(QuestionTarget::Assistant(Box::default())),
                    _ => Err(serde::de::Error::unknown_variant(v, &["user", "assistant"])),
                }
            }

            fn visit_map<A>(self, map: A) -> Result<QuestionTarget, A::Error>
            where
                A: serde::de::MapAccess<'de>,
            {
                let config = PartialAssistantConfig::deserialize(
                    serde::de::value::MapAccessDeserializer::new(map),
                )?;
                Ok(QuestionTarget::Assistant(Box::new(config)))
            }
        }

        deserializer.deserialize_any(QuestionTargetVisitor)
    }
}

impl schematic::Schematic for QuestionTarget {
    fn schema_name() -> Option<String> {
        Some("QuestionTarget".to_owned())
    }

    fn build_schema(mut schema: schematic::SchemaBuilder) -> schematic::Schema {
        use schematic::schema::{EnumType, LiteralValue, UnionType};

        schema.union(UnionType::new_any([
            schema.nest().enumerable(EnumType::new([
                LiteralValue::String("user".into()),
                LiteralValue::String("assistant".into()),
            ])),
            schema.infer::<PartialAssistantConfig>(),
        ]))
    }
}

/// CLI directive scope: which kind of `--tool` / `--no-tool` directive is being
/// matched against a tool's [`AllowToggle`] policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToggleScope {
    /// `-t` / `-T` with no argument: a bulk directive over all tools.
    Bulk,

    /// `-t NAME` / `-T NAME`: a directive naming a single tool.
    Named,

    /// `-t GROUP` / `-T GROUP`: a directive naming a tool group.
    ///
    /// Reserved for tool-group parsing; no directive parser produces this value
    /// yet.
    NamedGroup,
}

/// Which CLI directives may flip a tool's enabled state.
///
/// - `any`: any `--tool` / `--no-tool` directive may flip the state (default).
/// - `never`: no directive may flip the state (the tool is locked).
/// - `if_named`: only a directive that names the tool may flip the state.
/// - `if_named_or_group`: a directive naming the tool or its group may flip the
///   state.
///   Behaves like `if_named` until tool groups land.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, ConfigEnum)]
#[serde(rename_all = "snake_case")]
pub enum AllowToggle {
    /// Any directive may flip the tool's state.
    #[default]
    #[serde(rename = "any")]
    Always,

    /// No directive may flip the tool's state.
    Never,

    /// Only named-tool directives may flip the tool's state.
    IfNamed,

    /// Named-tool or named-group directives may flip the tool's state.
    IfNamedOrGroup,
}

/// Resolved tool activation: the effective enabled `state` and the
/// `allow_toggle` policy that governs which CLI directives may change it.
///
/// Produced on demand by the effective-enable resolver
/// ([`ToolConfigWithDefaults::effective_enable`] and
/// [`PartialEnableConfig::effective`]); never stored directly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Enable {
    /// Whether the tool is enabled.
    pub state: bool,

    /// Which directive scopes may flip [`state`].
    ///
    /// [`state`]: Self::state
    pub allow_toggle: AllowToggle,
}

impl Enable {
    /// Returns whether the tool is enabled.
    #[must_use]
    pub const fn is_enabled(&self) -> bool {
        self.state
    }

    /// Returns whether the tool's state is locked, i.e. no directive may flip
    /// it.
    #[must_use]
    pub const fn is_locked(&self) -> bool {
        matches!(self.allow_toggle, AllowToggle::Never)
    }

    /// Returns whether a directive of the given [`ToggleScope`] may flip the
    /// tool's state under the current [`AllowToggle`] policy.
    ///
    /// `Always` accepts every scope, `Never` accepts none, `IfNamed` accepts
    /// only named-tool directives, and `IfNamedOrGroup` accepts named-tool and
    /// named-group directives.
    #[must_use]
    pub const fn accepts(&self, scope: ToggleScope) -> bool {
        matches!(
            (self.allow_toggle, scope),
            (AllowToggle::Always, _)
                | (AllowToggle::IfNamed, ToggleScope::Named)
                | (
                    AllowToggle::IfNamedOrGroup,
                    ToggleScope::Named | ToggleScope::NamedGroup
                )
        )
    }
}

/// Tool activation: whether the tool is enabled and which `--tool` /
/// `--no-tool` CLI directives may change that.
///
/// ```toml
/// # Bool shorthand: enabled / disabled, freely toggleable.
/// enable = true
///
/// # Table form for finer control. An omitted field inherits from the global
/// # `[conversation.tools.'*']` defaults, then falls back to enabled / `any`.
/// enable = { state = false, allow_toggle = "if_named" }  # off unless named
/// enable = { state = true, allow_toggle = "never" }      # always on, locked
/// ```
///
/// The legacy strings `"on"`, `"off"`, `"always"` (= locked-on), and
/// `"explicit"` (= off-unless-named) are still accepted on input.
#[derive(Debug, Clone, PartialEq, Config)]
#[config(rename_all = "snake_case", no_deserialize_derive)]
pub struct EnableConfig {
    /// Whether the tool is enabled.
    ///
    /// When unset, inherits from the `[conversation.tools.'*']` defaults, then
    /// falls back to enabled.
    pub state: Option<bool>,

    /// Which `--tool` / `--no-tool` directives may flip `state`.
    ///
    /// - `any`: any directive may flip it (the default).
    /// - `never`: no directive may flip it (the tool is locked).
    /// - `if_named`: only a directive that names the tool may flip it.
    /// - `if_named_or_group`: a directive naming the tool or its group may flip
    ///   it.
    ///
    /// When unset, inherits from the `[conversation.tools.'*']` defaults, then
    /// falls back to `any`.
    pub allow_toggle: Option<AllowToggle>,
}

impl PartialEnableConfig {
    /// Enabled, freely toggleable.
    pub const ON: Self = Self {
        state: Some(true),
        allow_toggle: Some(AllowToggle::Always),
    };

    /// Disabled, freely toggleable.
    pub const OFF: Self = Self {
        state: Some(false),
        allow_toggle: Some(AllowToggle::Always),
    };

    /// Enabled, cannot be toggled off.
    pub const LOCKED_ON: Self = Self {
        state: Some(true),
        allow_toggle: Some(AllowToggle::Never),
    };

    /// Disabled, cannot be toggled on.
    pub const LOCKED_OFF: Self = Self {
        state: Some(false),
        allow_toggle: Some(AllowToggle::Never),
    };

    /// Resolve this per-tool value against a `defaults` layer into the
    /// effective [`Enable`].
    ///
    /// Each field is filled from `self`, then `defaults`, then the hardcoded
    /// fallback (`state = true`, `allow_toggle = any`).
    #[must_use]
    pub fn effective(&self, defaults: &Self) -> Enable {
        Enable {
            state: self.state.or(defaults.state).unwrap_or(true),
            allow_toggle: self
                .allow_toggle
                .or(defaults.allow_toggle)
                .unwrap_or_default(),
        }
    }
}

impl From<bool> for PartialEnableConfig {
    fn from(state: bool) -> Self {
        Self {
            state: Some(state),
            allow_toggle: Some(AllowToggle::Always),
        }
    }
}

impl FromStr for PartialEnableConfig {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "true" | "on" => Self::ON,
            "false" | "off" => Self::OFF,
            "always" => Self::LOCKED_ON,
            "explicit" => Self {
                state: Some(false),
                allow_toggle: Some(AllowToggle::IfNamed),
            },
            _ => {
                return Err(format!(
                    "invalid enable value: '{s}', expected a boolean, one of \"on\", \"off\", \
                     \"explicit\", \"always\", or a {{ state, allow_toggle }} table"
                ));
            }
        })
    }
}

impl AssignKeyValue for PartialEnableConfig {
    fn assign(&mut self, kv: KvAssignment) -> AssignResult {
        match kv.key_string().as_str() {
            "" => *self = kv.try_object_bool_or_from_str()?,
            "state" => self.state = kv.try_some_bool()?,
            "allow_toggle" => self.allow_toggle = kv.try_some_from_str()?,
            _ => return missing_key(&kv),
        }

        Ok(())
    }
}

impl PartialConfigDelta for PartialEnableConfig {
    fn delta(&self, next: Self) -> Self {
        Self {
            state: delta_opt(self.state.as_ref(), next.state),
            allow_toggle: delta_opt(self.allow_toggle.as_ref(), next.allow_toggle),
        }
    }
}

impl ToPartial for EnableConfig {
    fn to_partial(&self) -> Self::Partial {
        PartialEnableConfig {
            state: self.state,
            allow_toggle: self.allow_toggle,
        }
    }
}

/// Accept a bool, a legacy string, or a `{ state, allow_toggle }` table on
/// input.
/// Output is always the table form (auto-derived), mirroring how
/// [`ModelIdConfig`] accepts a `provider/name` string but writes a table.
///
/// [`ModelIdConfig`]: crate::model::id::ModelIdConfig
impl<'de> Deserialize<'de> for PartialEnableConfig {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct EnableVisitor;

        impl<'de> serde::de::Visitor<'de> for EnableVisitor {
            type Value = PartialEnableConfig;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(
                    "a boolean, one of \"on\"/\"off\"/\"explicit\"/\"always\", or a { state, \
                     allow_toggle } table",
                )
            }

            fn visit_bool<E: serde::de::Error>(self, v: bool) -> Result<Self::Value, E> {
                Ok(PartialEnableConfig::from(v))
            }

            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Self::Value, E> {
                v.parse().map_err(serde::de::Error::custom)
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: serde::de::MapAccess<'de>,
            {
                let mut state: Option<bool> = None;
                let mut allow_toggle: Option<AllowToggle> = None;

                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "state" => {
                            if state.is_some() {
                                return Err(serde::de::Error::duplicate_field("state"));
                            }
                            state = Some(map.next_value()?);
                        }
                        "allow_toggle" => {
                            if allow_toggle.is_some() {
                                return Err(serde::de::Error::duplicate_field("allow_toggle"));
                            }
                            allow_toggle = Some(map.next_value()?);
                        }
                        other => {
                            return Err(serde::de::Error::unknown_field(other, &[
                                "state",
                                "allow_toggle",
                            ]));
                        }
                    }
                }

                Ok(PartialEnableConfig {
                    state,
                    allow_toggle,
                })
            }
        }

        deserializer.deserialize_any(EnableVisitor)
    }
}

#[cfg(test)]
#[path = "tool_tests.rs"]
mod tests;
