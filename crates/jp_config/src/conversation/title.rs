//! Title configuration for conversations.

pub mod generate;

use schematic::Config;

use crate::{
    assignment::{AssignKeyValue, AssignResult, KvAssignment, missing_key},
    conversation::title::generate::{GenerateConfig, PartialGenerateConfig},
    delta::{PartialConfigDelta, delta_opt},
    fill::FillDefaults,
    partial::{ToPartial, partial_opt},
};

/// Title configuration.
#[derive(Debug, Clone, PartialEq, Config)]
#[config(rename_all = "snake_case")]
pub struct TitleConfig {
    /// Title generation configuration.
    ///
    /// Configures how and when titles are automatically generated for new
    /// conversations.
    #[setting(nested)]
    pub generate: GenerateConfig,

    /// Whether to use a leading markdown heading as the conversation title.
    ///
    /// Defaults to `true`.
    /// When the first message of a new conversation starts with a markdown
    /// heading (`# Title`, or a setext `Title` underlined with `===`), that
    /// heading becomes the title and no title is generated.
    /// Set to `false` to always rely on title generation instead.
    ///
    /// An explicit `--title` or `--no-title` flag takes precedence over both.
    #[setting(default = true)]
    pub from_header: bool,
}

impl AssignKeyValue for PartialTitleConfig {
    fn assign(&mut self, mut kv: KvAssignment) -> AssignResult {
        match kv.key_string().as_str() {
            "" => kv.try_merge_object(self)?,
            "from_header" => self.from_header = kv.try_some_bool()?,
            _ if kv.p("generate") => self.generate.assign(kv)?,
            _ => return missing_key(&kv),
        }

        Ok(())
    }
}

impl PartialConfigDelta for PartialTitleConfig {
    fn delta(&self, next: Self) -> Self {
        Self {
            generate: self.generate.delta(next.generate),
            from_header: delta_opt(self.from_header.as_ref(), next.from_header),
        }
    }
}

impl FillDefaults for PartialTitleConfig {
    fn fill_from(self, defaults: Self) -> Self {
        Self {
            generate: self.generate.fill_from(defaults.generate),
            from_header: self.from_header.or(defaults.from_header),
        }
    }
}

impl ToPartial for TitleConfig {
    fn to_partial(&self) -> Self::Partial {
        Self::Partial {
            generate: self.generate.to_partial(),
            from_header: partial_opt(&self.from_header, None),
        }
    }
}
