//! Markdown rendering configuration.

use schematic::{Config, ConfigEnum};
use serde::{Deserialize, Serialize};

use crate::{
    assignment::{AssignKeyValue, AssignResult, KvAssignment, missing_key},
    delta::{PartialConfigDelta, delta_opt},
    partial::{ToPartial, partial_opt, partial_opts},
};

/// Controls how colors are rendered in terminal output.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize, ConfigEnum)]
#[serde(rename_all = "lowercase")]
pub enum ColorModeConfig {
    /// Detect from `COLORTERM` / `TERM` environment variables.
    #[default]
    Auto,

    /// Force 24-bit RGB escape sequences.
    Truecolor,

    /// Force 256-color palette escape sequences.
    #[serde(rename = "256")]
    #[variant(value = "256")]
    Ansi256,

    /// No color escapes — plain text only.
    Plain,
}

/// Controls how horizontal rules (`---`) are rendered in terminal output.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize, ConfigEnum)]
#[serde(rename_all = "lowercase")]
pub enum HrStyle {
    /// Render the original markdown (`---`).
    Markdown,

    /// Render a continuous unicode horizontal line (`─`) spanning the full line
    /// width (based on `wrap_width`).
    #[default]
    Line,
}

impl HrStyle {
    /// Returns `true` if this is a line style.
    #[must_use]
    pub const fn is_line(&self) -> bool {
        matches!(self, Self::Line)
    }
}

/// Markdown rendering configuration.
#[derive(Debug, Clone, PartialEq, Config)]
#[config(rename_all = "snake_case")]
pub struct MarkdownConfig {
    /// Maximum line width for wrapping paragraph text.
    ///
    /// Set to `0` to disable wrapping entirely.
    #[setting(default = 80)]
    pub wrap_width: usize,

    /// Maximum visual width for a single table column.
    ///
    /// Cells exceeding this width are wrapped over multiple lines.
    ///
    /// Set to `0` to disable wrapping.
    #[setting(default = 40)]
    pub table_max_column_width: usize,

    /// Syntax highlighting theme for code blocks.
    ///
    /// Uses `bat` / `syntect` theme names (e.g. `"Monokai Extended"`,
    /// `"OneHalfDark"`, `"base16"`).
    #[setting(default = "gruvbox-dark")]
    pub theme: Option<String>,

    /// How horizontal rules are rendered in terminal output.
    ///
    /// - `markdown`: render the original CommonMark syntax (`---`).
    /// - `line`: render a continuous unicode horizontal line (`─`) spanning the
    ///   [`Self::wrap_width`].
    #[setting(default)]
    pub hr_style: HrStyle,

    /// Color mode for syntax highlighting and theme-derived colors.
    ///
    /// - `auto`: detect from `COLORTERM` / `TERM` environment variables.
    /// - `truecolor`: force 24-bit RGB escape sequences.
    /// - `256`: force 256-color palette escape sequences.
    /// - `plain`: no color escapes at all.
    #[setting(default)]
    pub color_mode: ColorModeConfig,
}

impl AssignKeyValue for PartialMarkdownConfig {
    fn assign(&mut self, kv: KvAssignment) -> AssignResult {
        match kv.key_string().as_str() {
            "" => kv.try_merge_object(self)?,
            "wrap_width" => self.wrap_width = kv.try_some_from_str()?,
            "table_max_column_width" => {
                self.table_max_column_width = kv.try_some_from_str()?;
            }
            "theme" => self.theme = kv.try_some_from_str()?,
            "hr_style" => self.hr_style = kv.try_some_from_str()?,
            "color_mode" => self.color_mode = kv.try_some_from_str()?,
            _ => return missing_key(&kv),
        }

        Ok(())
    }
}

impl PartialConfigDelta for PartialMarkdownConfig {
    fn delta(&self, next: Self) -> Self {
        Self {
            wrap_width: delta_opt(self.wrap_width.as_ref(), next.wrap_width),
            table_max_column_width: delta_opt(
                self.table_max_column_width.as_ref(),
                next.table_max_column_width,
            ),
            theme: delta_opt(self.theme.as_ref(), next.theme),
            hr_style: delta_opt(self.hr_style.as_ref(), next.hr_style),
            color_mode: delta_opt(self.color_mode.as_ref(), next.color_mode),
        }
    }
}

impl ToPartial for MarkdownConfig {
    fn to_partial(&self) -> Self::Partial {
        let defaults = Self::Partial::default();

        Self::Partial {
            wrap_width: partial_opt(&self.wrap_width, defaults.wrap_width),
            table_max_column_width: partial_opt(
                &self.table_max_column_width,
                defaults.table_max_column_width,
            ),
            theme: partial_opts(self.theme.as_ref(), defaults.theme),
            hr_style: partial_opt(&self.hr_style, defaults.hr_style),
            color_mode: partial_opt(&self.color_mode, defaults.color_mode),
        }
    }
}
