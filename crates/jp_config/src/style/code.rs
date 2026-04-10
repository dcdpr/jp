//! Code block styling configuration.

use schematic::Config;

use crate::{
    assignment::{AssignKeyValue, AssignResult, KvAssignment, missing_key},
    delta::{PartialConfigDelta, delta_opt},
    fill::FillDefaults,
    partial::{ToPartial, partial_opt},
    style::LinkStyle,
};

/// Code style configuration.
#[derive(Debug, Clone, PartialEq, Config)]
#[config(rename_all = "snake_case")]
pub struct CodeConfig {
    /// Whether to colorize code blocks.
    #[setting(default = true)]
    pub color: bool,

    /// Show line numbers in code blocks.
    #[setting(default = false)]
    pub line_numbers: bool,

    /// Show a link to the file containing the source code in code blocks.
    ///
    /// - `off`: Do not show the link.
    /// - `full`: Show the full file path.
    /// - `osc8`: Show a clickable link (OSC8 escape sequence).
    ///
    /// See: <https://gist.github.com/egmontkob/eb114294efbcd5adb1944c9f3cb5feda>
    #[setting(default = "osc8")]
    pub file_link: LinkStyle,

    /// Similar to `file_link`, but adds a link with the scheme `copy://`.
    ///
    /// If your terminal (configuration) supports it, this allows you to copy
    /// the code block contents to your clipboard.
    ///
    /// Defaults to `off`, because no terminal supports it out of the box.
    ///
    /// Here is an example of how to make this work using
    /// [WezTerm](https://wezfurlong.org/wezterm/) on macOS:
    ///
    /// ```lua
    /// local wezterm = require("wezterm")
    ///
    /// wezterm.on("open-uri", function(_, pane, uri)
    ///   if uri:find("^copy:") == 1 and not pane:is_alt_screen_active() then
    ///     local url = wezterm.url.parse(uri)
    ///     pane:send_text("pbcopy < " .. url.file_path .. "\r")
    ///   end
    /// end)
    /// ```
    #[setting(default = "off")]
    pub copy_link: LinkStyle,
}

impl AssignKeyValue for PartialCodeConfig {
    fn assign(&mut self, kv: KvAssignment) -> AssignResult {
        match kv.key_string().as_str() {
            "" => kv.try_merge_object(self)?,
            "color" => self.color = kv.try_some_bool()?,
            "line_numbers" => self.line_numbers = kv.try_some_bool()?,
            "file_link" => self.file_link = kv.try_some_from_str()?,
            "copy_link" => self.copy_link = kv.try_some_from_str()?,
            _ => return missing_key(&kv),
        }

        Ok(())
    }
}

impl PartialConfigDelta for PartialCodeConfig {
    fn delta(&self, next: Self) -> Self {
        Self {
            color: delta_opt(self.color.as_ref(), next.color),
            line_numbers: delta_opt(self.line_numbers.as_ref(), next.line_numbers),
            file_link: delta_opt(self.file_link.as_ref(), next.file_link),
            copy_link: delta_opt(self.copy_link.as_ref(), next.copy_link),
        }
    }
}

impl FillDefaults for PartialCodeConfig {
    fn fill_from(self, defaults: Self) -> Self {
        Self {
            color: self.color.or(defaults.color),
            line_numbers: self.line_numbers.or(defaults.line_numbers),
            file_link: self.file_link.or(defaults.file_link),
            copy_link: self.copy_link.or(defaults.copy_link),
        }
    }
}

impl ToPartial for CodeConfig {
    fn to_partial(&self) -> Self::Partial {
        let defaults = Self::Partial::default();

        Self::Partial {
            color: partial_opt(&self.color, defaults.color),
            line_numbers: partial_opt(&self.line_numbers, defaults.line_numbers),
            file_link: partial_opt(&self.file_link, defaults.file_link),
            copy_link: partial_opt(&self.copy_link, defaults.copy_link),
        }
    }
}
