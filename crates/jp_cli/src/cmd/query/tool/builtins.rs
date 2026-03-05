use indexmap::IndexMap;
use jp_config::conversation::tool::{
    Enable, PartialOneOrManyTypes, PartialToolConfig, PartialToolParameterConfig, RunMode,
    ToolSource,
    style::{InlineResults, LinkStyle, ParametersStyle, PartialDisplayStyleConfig},
};

pub fn all() -> IndexMap<String, PartialToolConfig> {
    IndexMap::from([("describe_tools".to_owned(), describe_tools())])
}

/// Returns the built-in `describe_tools` tool configuration.
#[must_use]
pub fn describe_tools() -> PartialToolConfig {
    PartialToolConfig {
        source: Some(ToolSource::Builtin { tool: None }),
        enable: Some(Enable::Always),
        description: Some(
            "Get detailed descriptions and usage examples for one or more tools.".to_owned(),
        ),
        parameters: IndexMap::from([("tools".to_owned(), PartialToolParameterConfig {
            kind: PartialOneOrManyTypes::One("array".to_owned()),
            required: Some(true),
            description: Some("Array of tool names to describe".to_owned()),
            items: Some(Box::new(PartialToolParameterConfig {
                kind: PartialOneOrManyTypes::One("string".to_owned()),
                ..Default::default()
            })),
            ..Default::default()
        })]),
        run: Some(RunMode::Unattended),
        style: Some(PartialDisplayStyleConfig {
            hidden: Some(true),
            inline_results: Some(InlineResults::Off),
            results_file_link: Some(LinkStyle::Off),
            parameters: Some(ParametersStyle::Off),
        }),
        ..Default::default()
    }
}
