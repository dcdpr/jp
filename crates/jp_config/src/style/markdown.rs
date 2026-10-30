//! Markdown rendering configuration.

use schematic::{Config, ConfigEnum};
use serde::{Deserialize, Serialize};

use crate::{
    assignment::{AssignKeyValue, AssignResult, KvAssignment, missing_key},
    delta::{PartialConfigDelta, delta_opt, delta_opt_at, path},
    fill::FillDefaults,
    partial::{ToPartial, partial_opt, partial_opts},
    types::color::Color,
};

/// Controls how horizontal rules (`---`) are rendered in terminal output.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize, ConfigEnum)]
#[serde(rename_all = "lowercase")]
pub enum HrStyle {
    /// Render the original markdown (`---`).
    Markdown,

    /// Render a continuous unicode horizontal line (`─`) spanning the full
    /// line width (based on `wrap_width`).
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
    /// Defaults to `80`.
    /// Set to `0` to disable wrapping entirely.
    ///
    /// This is a reading-comfort preference, so a wider output area does not
    /// widen it.
    /// A narrower one caps it: text is never wrapped past the columns
    /// available, since the terminal would then wrap it a second time.
    #[setting(default = 80)]
    pub wrap_width: usize,

    /// Upper bound on the visual width of a single table column.
    ///
    /// Defaults to `40`.
    /// Set to `0` to leave columns as wide as their content.
    ///
    /// Body cells exceeding their column's width are wrapped over multiple
    /// lines.
    /// A line continuing the row above opens with `┆` instead of `|`, so a
    /// wrapped row reads as one row rather than several.
    /// A header cell is cut short with `…` rather than wrapped, so the row of
    /// dashes stays directly beneath the header and the table survives being
    /// copied out of the terminal into a markdown document.
    /// A column can end up narrower than this: a table wider than the terminal
    /// has its widest columns narrowed until it fits, so the terminal does not
    /// break the rows apart.
    /// Columns never narrow below three characters, so `1` and `2` behave as
    /// `3`, and a table with more columns than fit at that minimum is rendered
    /// at the minimum and overflows the terminal.
    /// Tables in piped or redirected output are not fitted, since there is no
    /// width to fit them to — unless `--width` supplies one.
    #[setting(default = 40)]
    pub table_max_column_width: usize,

    /// Whether the continuation lines of a wrapped table row open with `┆`.
    ///
    /// Defaults to `true`.
    /// Set to `false` to open every line with `|`.
    ///
    /// A cell wrapped over several lines otherwise reads as several one-line
    /// rows, since nothing distinguishes the start of a row from the middle of
    /// one.
    /// Only the line's opening delimiter changes, so a table copied out of the
    /// terminal into a markdown document still splits into the right columns.
    #[setting(default = true)]
    pub table_continuation_edge: bool,

    /// Syntax highlighting theme for code blocks.
    ///
    /// Uses `bat` / `syntect` theme names (e.g. `"Monokai Extended"`,
    /// `"OneHalfDark"`, `"base16"`).
    #[setting(default = "gruvbox-dark")]
    pub theme: Option<String>,

    /// How horizontal rules are rendered in terminal output.
    ///
    /// - `markdown`: render the original CommonMark syntax (`---`).
    /// - `line`: render a continuous unicode horizontal line (`─`) spanning
    ///   `wrap_width`.
    #[setting(default)]
    pub hr_style: HrStyle,

    /// Per-element visual style overrides.
    ///
    /// Configures foreground and background colors for individual markdown
    /// elements such as `$..$` math spans.
    ///
    /// ```toml
    /// [style.markdown.elements.math]
    /// fg = "yellow"
    /// ```
    #[setting(nested)]
    pub elements: ElementsConfig,
}

/// Per-element visual style overrides for rendered markdown.
///
/// ```toml
/// [style.markdown.elements.math]
/// fg = "yellow"
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Config)]
#[config(rename_all = "snake_case")]
pub struct ElementsConfig {
    /// Style for inline and display math spans (`$..$`, `$$..$$`).
    ///
    /// When both `fg` and `bg` are unset, math spans are rendered with no
    /// additional color.
    #[setting(nested)]
    pub math: ElementStyleConfig,
}

/// Visual style overrides for a markdown element.
#[derive(Debug, Clone, PartialEq, Eq, Config)]
#[config(rename_all = "snake_case")]
pub struct ElementStyleConfig {
    /// Foreground color for the element.
    ///
    /// When unset, the surrounding foreground color is kept.
    /// Accepts an ANSI 256-color index (e.g. `220`), a named ANSI color (e.g.
    /// `"yellow"` or `"bright_yellow"`), or a hex RGB string (e.g.
    /// `"#fabd2f"`).
    pub fg: Option<Color>,

    /// Background color for the element.
    ///
    /// When unset, the surrounding background color is kept.
    /// Accepts an ANSI 256-color index (e.g. `236`), a named ANSI color (e.g.
    /// `"yellow"` or `"bright_yellow"`), or a hex RGB string (e.g.
    /// `"#3c3836"`).
    pub bg: Option<Color>,
}

impl AssignKeyValue for PartialElementStyleConfig {
    fn assign(&mut self, kv: KvAssignment) -> AssignResult {
        match kv.key_string().as_str() {
            "" => kv.try_merge_object(self)?,
            "fg" => self.fg = kv.try_some_number_or_from_str()?,
            "bg" => self.bg = kv.try_some_number_or_from_str()?,
            _ => return missing_key(&kv),
        }

        Ok(())
    }
}

impl PartialConfigDelta for PartialElementStyleConfig {
    fn delta(&self, next: Self) -> Self {
        Self {
            fg: delta_opt(self.fg.as_ref(), next.fg),
            bg: delta_opt(self.bg.as_ref(), next.bg),
        }
    }

    fn delta_with_unsets(&self, next: Self, prefix: &str, unsets: &mut Vec<String>) -> Self {
        Self {
            fg: delta_opt_at(&path(prefix, "fg"), self.fg.as_ref(), next.fg, unsets),
            bg: delta_opt_at(&path(prefix, "bg"), self.bg.as_ref(), next.bg, unsets),
        }
    }
}

impl FillDefaults for PartialElementStyleConfig {
    fn fill_from(self, defaults: Self) -> Self {
        Self {
            fg: self.fg.or(defaults.fg),
            bg: self.bg.or(defaults.bg),
        }
    }
}

impl ToPartial for ElementStyleConfig {
    fn to_partial(&self) -> Self::Partial {
        let defaults = Self::Partial::default();

        Self::Partial {
            fg: delta_opt(defaults.fg.as_ref(), self.fg),
            bg: delta_opt(defaults.bg.as_ref(), self.bg),
        }
    }
}

impl AssignKeyValue for PartialElementsConfig {
    fn assign(&mut self, mut kv: KvAssignment) -> AssignResult {
        match kv.key_string().as_str() {
            "" => kv.try_merge_object(self)?,
            _ if kv.p("math") => self.math.assign(kv)?,
            _ => return missing_key(&kv),
        }

        Ok(())
    }
}

impl PartialConfigDelta for PartialElementsConfig {
    fn delta(&self, next: Self) -> Self {
        Self {
            math: self.math.delta(next.math),
        }
    }

    fn delta_with_unsets(&self, next: Self, prefix: &str, unsets: &mut Vec<String>) -> Self {
        Self {
            math: self
                .math
                .delta_with_unsets(next.math, &path(prefix, "math"), unsets),
        }
    }
}

impl FillDefaults for PartialElementsConfig {
    fn fill_from(self, defaults: Self) -> Self {
        Self {
            math: self.math.fill_from(defaults.math),
        }
    }
}

impl ToPartial for ElementsConfig {
    fn to_partial(&self) -> Self::Partial {
        Self::Partial {
            math: self.math.to_partial(),
        }
    }
}

impl AssignKeyValue for PartialMarkdownConfig {
    fn assign(&mut self, mut kv: KvAssignment) -> AssignResult {
        match kv.key_string().as_str() {
            "" => kv.try_merge_object(self)?,
            "wrap_width" => self.wrap_width = kv.try_some_from_str()?,
            "table_max_column_width" => {
                self.table_max_column_width = kv.try_some_from_str()?;
            }
            "table_continuation_edge" => {
                self.table_continuation_edge = kv.try_some_from_str()?;
            }
            "theme" => self.theme = kv.try_some_from_str()?,
            "hr_style" => self.hr_style = kv.try_some_from_str()?,
            _ if kv.p("elements") => self.elements.assign(kv)?,
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
            table_continuation_edge: delta_opt(
                self.table_continuation_edge.as_ref(),
                next.table_continuation_edge,
            ),
            theme: delta_opt(self.theme.as_ref(), next.theme),
            hr_style: delta_opt(self.hr_style.as_ref(), next.hr_style),
            elements: self.elements.delta(next.elements),
        }
    }

    fn delta_with_unsets(&self, next: Self, prefix: &str, unsets: &mut Vec<String>) -> Self {
        Self {
            wrap_width: delta_opt(self.wrap_width.as_ref(), next.wrap_width),
            table_max_column_width: delta_opt(
                self.table_max_column_width.as_ref(),
                next.table_max_column_width,
            ),
            table_continuation_edge: delta_opt(
                self.table_continuation_edge.as_ref(),
                next.table_continuation_edge,
            ),
            theme: delta_opt(self.theme.as_ref(), next.theme),
            hr_style: delta_opt(self.hr_style.as_ref(), next.hr_style),
            elements: self.elements.delta_with_unsets(
                next.elements,
                &path(prefix, "elements"),
                unsets,
            ),
        }
    }
}

impl FillDefaults for PartialMarkdownConfig {
    fn fill_from(self, defaults: Self) -> Self {
        Self {
            wrap_width: self.wrap_width.or(defaults.wrap_width),
            table_max_column_width: self
                .table_max_column_width
                .or(defaults.table_max_column_width),
            table_continuation_edge: self
                .table_continuation_edge
                .or(defaults.table_continuation_edge),
            theme: self.theme.or(defaults.theme),
            hr_style: self.hr_style.or(defaults.hr_style),
            elements: self.elements.fill_from(defaults.elements),
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
            table_continuation_edge: partial_opt(
                &self.table_continuation_edge,
                defaults.table_continuation_edge,
            ),
            theme: partial_opts(self.theme.as_ref(), defaults.theme),
            hr_style: partial_opt(&self.hr_style, defaults.hr_style),
            elements: self.elements.to_partial(),
        }
    }
}
