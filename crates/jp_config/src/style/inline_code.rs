//! Inline code span styling configuration.

use schematic::Config;

use crate::{
    assignment::{AssignKeyValue, AssignResult, KvAssignment, missing_key},
    delta::{PartialConfigDelta, delta_opt},
    partial::ToPartial,
    types::color::Color,
};

/// Inline code span style configuration.
///
/// Controls the visual appearance of inline code (`` `like this` ``) in
/// terminal output, independently from fenced code block styling.
#[derive(Debug, Clone, PartialEq, Config)]
#[config(rename_all = "snake_case")]
pub struct InlineCodeConfig {
    /// Background color for inline code spans.
    ///
    /// Overrides the background derived from the syntax highlighting theme.
    /// Accepts either an ANSI 256-color index (e.g. `236`) or a hex RGB
    /// string (e.g. `"#504945"`).
    ///
    /// When unset, the theme's background color is used (the default).
    pub background: Option<Color>,
}

impl AssignKeyValue for PartialInlineCodeConfig {
    fn assign(&mut self, kv: KvAssignment) -> AssignResult {
        match kv.key_string().as_str() {
            "" => kv.try_merge_object(self)?,
            "background" => self.background = kv.try_some_number_or_from_str()?,
            _ => return missing_key(&kv),
        }

        Ok(())
    }
}

impl PartialConfigDelta for PartialInlineCodeConfig {
    fn delta(&self, next: Self) -> Self {
        Self {
            background: delta_opt(self.background.as_ref(), next.background),
        }
    }
}

impl ToPartial for InlineCodeConfig {
    fn to_partial(&self) -> Self::Partial {
        let defaults = Self::Partial::default();

        Self::Partial {
            background: delta_opt(defaults.background.as_ref(), self.background),
        }
    }
}
