//! Tool configuration for conversations.

use std::str::FromStr;

use indexmap::IndexMap;
use schematic::{Config, ConfigEnum};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use tracing::warn;

use crate::{
    assignment::{missing_key, AssignKeyValue, AssignResult, KvAssignment},
    conversation::tool::style::{DisplayStyleConfig, PartialDisplayStyleConfig},
    delta::{delta_opt, delta_opt_partial, delta_opt_vec, delta_vec, PartialConfigDelta},
    partial::{partial_opt, partial_opt_config, partial_opts, ToPartial},
    util::merge_nested_indexmap,
    BoxedError,
};

pub mod style;

/// Tools configuration.
#[derive(Debug, Clone, Config)]
#[config(rename_all = "snake_case", allow_unknown_fields)]
pub struct ToolsConfig {
    /// Global config
    #[setting(nested, rename = "*")]
    pub defaults: ToolsDefaultsConfig,

    /// Tool config
    #[setting(nested, flatten, merge = merge_nested_indexmap)]
    tools: IndexMap<String, ToolConfig>,
}

impl AssignKeyValue for PartialToolsConfig {
    fn assign(&mut self, mut kv: KvAssignment) -> AssignResult {
        match kv.key_string().as_str() {
            "" => *self = kv.try_object()?,
            _ if kv.p("*") => self.defaults.assign(kv)?,
            _ => match kv.trim_prefix_any() {
                Some(tool_id) => self.tools.entry(tool_id).or_default().assign(kv)?,
                None => return missing_key(&kv),
            },
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
}

/// Tools defaults configuration.
#[derive(Debug, Clone, Config)]
#[config(rename_all = "snake_case")]
pub struct ToolsDefaultsConfig {
    /// How to run the tool.
    #[setting(required)]
    pub run: RunMode,

    /// How to deliver the results of the tool to the assistant.
    #[setting(default)]
    pub result: ResultMode,

    /// How to display the results of the tool in the terminal.
    #[setting(nested)]
    pub style: DisplayStyleConfig,
}

impl AssignKeyValue for PartialToolsDefaultsConfig {
    fn assign(&mut self, mut kv: KvAssignment) -> AssignResult {
        match kv.key_string().as_str() {
            "run" => self.run = kv.try_some_from_str()?,
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
            run: delta_opt(self.run.as_ref(), next.run),
            result: delta_opt(self.result.as_ref(), next.result),
            style: self.style.delta(next.style),
        }
    }
}

impl ToPartial for ToolsDefaultsConfig {
    fn to_partial(&self) -> Self::Partial {
        let defaults = Self::Partial::default();

        Self::Partial {
            run: partial_opt(&self.run, defaults.run),
            result: partial_opt(&self.result, defaults.result),
            style: self.style.to_partial(),
        }
    }
}

/// Tool configuration.
#[derive(Debug, Clone, Config)]
#[config(rename_all = "snake_case")]
pub struct ToolConfig {
    /// The source of the tool.
    #[setting(required)]
    pub source: ToolSource,

    /// Whether the tool is enabled.
    pub enable: Option<bool>,

    /// The command to run. Only used for local tools.
    #[setting(nested)]
    pub command: Option<ToolCommandConfigOrString>,

    /// The description of the tool. This will override any existing
    /// description, such as the one from an MCP server, or a built-in tool.
    pub description: Option<String>,

    /// The parameters expected by the tool.
    ///
    /// For `local` tools, omitting this will result in a tool that takes no
    /// parameters. For `mcp` or `builtin` tools, omitting this keeps the
    /// original parameters from the tool definition, but you can override
    /// existing parameters by specifying them here.
    ///
    /// Overriding parameters is allowed in narrow cases, such as flipping an
    /// argument from optional to required, defining an enumeration of allowed
    /// values, or forcing a specific value by setting a single enum value. You
    /// CANNOT change the type of the argument, its name, or any other
    /// properties that would break the tool's original argument expectations.
    #[setting(nested, merge = merge_nested_indexmap)]
    pub parameters: IndexMap<String, ToolParameterConfig>,

    /// How to run the tool.
    pub run: Option<RunMode>,

    /// How to deliver the results of the tool to the assistant.
    pub result: Option<ResultMode>,

    /// How to display the results of the tool in the terminal.
    #[setting(nested)]
    pub style: Option<DisplayStyleConfig>,
}

impl AssignKeyValue for PartialToolConfig {
    fn assign(&mut self, mut kv: KvAssignment) -> AssignResult {
        match kv.key_string().as_str() {
            "" => *self = kv.try_object()?,
            "source" => self.source = kv.try_some_from_str()?,
            "enable" => self.enable = kv.try_some_bool()?,
            _ if kv.p("command") => self.command.assign(kv)?,
            "description" => self.description = kv.try_some_string()?,
            "parameters" => self.parameters = kv.try_object()?,
            "run" => self.run = kv.try_some_from_str()?,
            "result" => self.result = kv.try_some_from_str()?,
            _ if kv.p("style") => self.style.assign(kv)?,
            _ => return missing_key(&kv),
        }

        Ok(())
    }
}

impl PartialConfigDelta for PartialToolConfig {
    fn delta(&self, next: Self) -> Self {
        Self {
            source: delta_opt(self.source.as_ref(), next.source),
            enable: delta_opt(self.enable.as_ref(), next.enable),
            command: delta_opt_partial(self.command.as_ref(), next.command),
            description: delta_opt(self.description.as_ref(), next.description),
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
            result: delta_opt(self.result.as_ref(), next.result),
            style: delta_opt_partial(self.style.as_ref(), next.style),
        }
    }
}

impl ToPartial for ToolConfig {
    fn to_partial(&self) -> Self::Partial {
        let defaults = Self::Partial::default();

        Self::Partial {
            source: partial_opt(&self.source, defaults.source),
            enable: partial_opts(self.enable.as_ref(), defaults.enable),
            command: partial_opt_config(self.command.as_ref(), defaults.command),
            description: partial_opts(self.description.as_ref(), defaults.description),
            parameters: self
                .parameters
                .iter()
                .map(|(k, v)| (k.clone(), v.to_partial()))
                .collect(),
            run: partial_opts(self.run.as_ref(), defaults.run),
            result: partial_opts(self.result.as_ref(), defaults.result),
            style: partial_opt_config(self.style.as_ref(), defaults.style),
        }
    }
}

/// Tool command configuration, either as a string or a complete configuration.
#[derive(Debug, Clone, Config)]
#[config(rename_all = "snake_case", serde(untagged))]
pub enum ToolCommandConfigOrString {
    /// A single string, which is interpreted as the command to run.
    String(String),

    /// A complete command configuration.
    #[setting(nested)]
    Config(ToolCommandConfig),
}

impl AssignKeyValue for PartialToolCommandConfigOrString {
    fn assign(&mut self, kv: KvAssignment) -> AssignResult {
        match kv.key_string().as_str() {
            "" => *self = kv.try_object_or_from_str()?,
            _ => match self {
                Self::String(_) => return missing_key(&kv),
                Self::Config(config) => config.assign(kv)?,
            },
        }

        Ok(())
    }
}

impl PartialConfigDelta for PartialToolCommandConfigOrString {
    fn delta(&self, next: Self) -> Self {
        match (self, next) {
            (Self::Config(prev), Self::Config(next)) => Self::Config(prev.delta(next)),
            (_, next) => next,
        }
    }
}

impl ToPartial for ToolCommandConfigOrString {
    fn to_partial(&self) -> Self::Partial {
        match self {
            Self::String(v) => Self::Partial::String(v.to_owned()),
            Self::Config(v) => Self::Partial::Config(v.to_partial()),
        }
    }
}

impl FromStr for PartialToolCommandConfigOrString {
    type Err = BoxedError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self::String(s.to_owned()))
    }
}

impl ToolCommandConfigOrString {
    /// Return the command configuration.
    ///
    /// If the configuration is a string, it is interpreted as a shell command.
    #[must_use]
    fn command(self) -> ToolCommandConfig {
        match self {
            Self::String(v) => {
                let mut iter = v.split_whitespace().map(str::to_owned);

                ToolCommandConfig {
                    program: iter.next().unwrap_or_default(),
                    args: iter.collect(),
                    shell: false,
                }
            }
            Self::Config(v) => v,
        }
    }
}

/// Tool command configuration.
#[derive(Debug, Clone, PartialEq, Config)]
#[config(rename_all = "snake_case")]
pub struct ToolCommandConfig {
    /// The program to run.
    pub program: String,

    /// The arguments to pass to the program.
    #[setting(default = vec![])]
    pub args: Vec<String>,

    /// Whether to run the command in a shell.
    ///
    /// If this is enabled, a shell will be invoked to run the command. This
    /// allows for things like piping and subshells.
    ///
    /// NOTE that setting this to `true` implies that JP will always ask for
    /// confirmation before running the tool, for security reasons.
    #[setting(default)]
    pub shell: bool,
}

impl AssignKeyValue for PartialToolCommandConfig {
    fn assign(&mut self, mut kv: KvAssignment) -> AssignResult {
        match kv.key_string().as_str() {
            "" => *self = kv.try_object()?,
            "program" => self.program = kv.try_some_string()?,
            _ if kv.p("args") => kv.try_some_vec_of_strings(&mut self.args)?,
            "shell" => self.shell = kv.try_some_bool()?,
            _ => return missing_key(&kv),
        }

        Ok(())
    }
}

impl PartialConfigDelta for PartialToolCommandConfig {
    fn delta(&self, next: Self) -> Self {
        Self {
            program: delta_opt(self.program.as_ref(), next.program),
            args: delta_opt_vec(self.args.as_ref(), next.args),
            shell: delta_opt(self.shell.as_ref(), next.shell),
        }
    }
}

impl ToPartial for ToolCommandConfig {
    fn to_partial(&self) -> Self::Partial {
        let defaults = Self::Partial::default();

        Self::Partial {
            program: partial_opt(&self.program, defaults.program),
            args: partial_opt(&self.args, defaults.args),
            shell: partial_opt(&self.shell, defaults.shell),
        }
    }
}

/// Tool parameter configuration.
#[derive(Debug, Clone, Config)]
#[config(rename_all = "snake_case")]
pub struct ToolParameterConfig {
    /// The type of the parameter.
    // TODO: Support `type` as an array of types.
    #[setting(nested, rename = "type")]
    pub kind: OneOrManyTypes,

    /// The default value of the parameter.
    pub default: Option<Value>,

    /// Whether the parameter is required.
    pub required: bool,

    /// Description of the parameter.
    pub description: Option<String>,

    /// A list of possible values for the parameter.
    #[setting(rename = "enum")]
    pub enumeration: Vec<Value>,

    /// Configuration for array items.
    #[setting(nested)]
    pub items: Option<ToolParameterItemsConfig>,
}

impl PartialConfigDelta for PartialToolParameterConfig {
    fn delta(&self, next: Self) -> Self {
        Self {
            kind: self.kind.delta(next.kind),
            default: delta_opt(self.default.as_ref(), next.default),
            required: delta_opt(self.required.as_ref(), next.required),
            description: delta_opt(self.description.as_ref(), next.description),
            enumeration: delta_opt(self.enumeration.as_ref(), next.enumeration),
            items: delta_opt_partial(self.items.as_ref(), next.items),
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
            description: partial_opts(self.description.as_ref(), defaults.description),
            enumeration: partial_opt(&self.enumeration, defaults.enumeration),
            items: partial_opt_config(self.items.as_ref(), defaults.items),
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
            Self::One(v) => v == type_,
            Self::Many(v) => v.iter().any(|v| v == type_),
        }
    }

    /// Return whether the type is exactly the given type.
    #[must_use]
    pub fn is_type(&self, type_: &str) -> bool {
        match self {
            Self::One(v) => v == type_,
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

        if let Some(description) = self.description.as_deref() {
            map.insert("description".to_owned(), description.into());
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

            if let Ok(v @ Value::Object(_)) = serde_json::to_value(items) {
                map.insert("items".to_owned(), v);
            } else {
                warn!("Unable to serialize `items` property");
            }
        }

        Value::Object(map)
    }
}

/// Tool parameter configuration.
#[derive(Debug, Clone, Serialize, Deserialize, Config)]
#[config(rename_all = "snake_case")]
pub struct ToolParameterItemsConfig {
    /// The type of the parameter array items.
    #[serde(rename = "type")]
    pub kind: String,
}

impl PartialConfigDelta for PartialToolParameterItemsConfig {
    fn delta(&self, next: Self) -> Self {
        Self {
            kind: delta_opt(self.kind.as_ref(), next.kind),
        }
    }
}

impl ToPartial for ToolParameterItemsConfig {
    fn to_partial(&self) -> Self::Partial {
        let defaults = Self::Partial::default();

        Self::Partial {
            kind: partial_opt(&self.kind, defaults.kind),
        }
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
        /// If not specified, all servers are searched, and the first one that
        /// contains the tool is used. If no server contains the tool, an error
        /// is returned.
        server: Option<String>,

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
                let mut s = "mcp".to_string();
                if let Some(server) = server {
                    s.push('.');
                    s.push_str(server);
                    if let Some(tool) = tool {
                        s.push('.');
                        s.push_str(tool);
                    }
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
                let (server, tool) = tool.map_or((None, None), |t| {
                    t.split_once('.')
                        .map(|(a, b)| (Some(a.to_owned()), Some(b.to_owned())))
                        .unwrap_or((Some(t), None))
                });

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
        schema.build()
    }
}

/// The run mode of a tool.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize, ConfigEnum)]
#[serde(rename_all = "lowercase")]
pub enum RunMode {
    /// Ask for confirmation before running the tool.
    #[default]
    Ask,

    /// Always run the tool, without asking for confirmation.
    Always,

    /// Open an editor to edit the tool call before running it.
    Edit,
}

/// How to deliver the results of the tool to the assistant.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize, ConfigEnum)]
#[serde(rename_all = "lowercase")]
pub enum ResultMode {
    /// Always deliver the results of the tool call.
    #[default]
    Always,

    /// Ask for confirmation before delivering the results of the tool call.
    Ask,

    /// Open an editor to edit the tool call result before delivering it.
    Edit,
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
    /// Return whether the tool is enabled.
    #[must_use]
    pub fn enable(&self) -> bool {
        // NOTE: We cannot define `#[setting(default = true)]` on the `enable`
        // field, because `AppConfig::default_values()` will result in an empty
        // `conversation.tools` map, which means that if we then merge that map
        // with the actual configuration, the `enable` field will still default
        // to `false`, because there is no default value set for any specific
        // entry in the map.
        self.tool.enable.unwrap_or(true)
    }

    /// Return the command to run the tool.
    #[must_use]
    pub fn command(&self) -> Option<ToolCommandConfig> {
        self.tool
            .command
            .clone()
            .map(ToolCommandConfigOrString::command)
    }

    /// Return the source of the tool.
    #[must_use]
    pub const fn source(&self) -> &ToolSource {
        &self.tool.source
    }

    /// Return the description of the tool.
    #[must_use]
    pub fn description(&self) -> Option<&str> {
        self.tool.description.as_deref()
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
}

#[cfg(test)]
mod tests {
    use std::assert_matches::assert_matches;

    use schematic::PartialConfig as _;

    use super::*;

    #[test]
    fn test_tools_config() {
        assert_matches!(PartialToolsConfig::default_values(&()), Ok(Some(_)));
        assert_matches!(PartialToolConfig::default_values(&()), Ok(Some(_)));

        let mut p = PartialToolsConfig::default_values(&()).unwrap().unwrap();

        p.tools.insert("cargo_check".to_owned(), PartialToolConfig {
            enable: Some(false),
            source: Some(ToolSource::Local { tool: None }),
            ..Default::default()
        });

        let kv = KvAssignment::try_from_cli("cargo_check.enable", "true").unwrap();
        p.assign(kv).unwrap();

        assert_eq!(
            p.tools,
            IndexMap::<_, _>::from_iter(vec![("cargo_check".to_owned(), PartialToolConfig {
                enable: Some(true),
                source: Some(ToolSource::Local { tool: None }),
                ..Default::default()
            })])
        );

        let kv = KvAssignment::try_from_cli("foo:", r#"{"source":"builtin"}"#).unwrap();
        p.assign(kv).unwrap();
        assert_eq!(
            p.tools,
            IndexMap::<_, _>::from_iter(vec![
                ("cargo_check".to_owned(), PartialToolConfig {
                    enable: Some(true),
                    source: Some(ToolSource::Local { tool: None }),
                    ..Default::default()
                }),
                ("foo".to_owned(), PartialToolConfig {
                    source: Some(ToolSource::Builtin { tool: None }),
                    ..Default::default()
                })
            ])
        );
    }

    #[test]
    fn test_tool_config_command() {
        let mut p = PartialToolConfig::default_values(&()).unwrap().unwrap();
        assert!(p.command.is_none());

        let kv = KvAssignment::try_from_cli("command", "cargo check").unwrap();
        p.assign(kv).unwrap();
        assert_eq!(
            p.command,
            Some(PartialToolCommandConfigOrString::String(
                "cargo check".to_owned()
            ))
        );

        let cfg = ToolCommandConfigOrString::from_partial(p.command.clone().unwrap()).unwrap();
        assert_eq!(cfg.command(), ToolCommandConfig {
            program: "cargo".to_owned(),
            args: vec!["check".to_owned()],
            shell: false,
        });

        let kv = KvAssignment::try_from_cli(
            "command:",
            r#"{"program":"cargo","args":["check", "--verbose"],"shell":true}"#,
        )
        .unwrap();
        p.assign(kv).unwrap();
        assert_eq!(
            p.command,
            Some(PartialToolCommandConfigOrString::Config(
                PartialToolCommandConfig {
                    program: Some("cargo".to_owned()),
                    args: Some(vec!["check".to_owned(), "--verbose".to_owned()]),
                    shell: Some(true),
                }
            ))
        );

        let cfg = ToolCommandConfigOrString::from_partial(p.command.unwrap()).unwrap();
        assert_eq!(cfg.command(), ToolCommandConfig {
            program: "cargo".to_owned(),
            args: vec!["check".to_owned(), "--verbose".to_owned()],
            shell: true,
        });
    }
}
