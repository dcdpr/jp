//! Code block styling configuration.

use schematic::Config;

use crate::{
    assignment::{missing_key, AssignKeyValue, AssignResult, KvAssignment},
    style::LinkStyle,
};

/// Code style configuration.
#[derive(Debug, Config)]
#[config(rename_all = "snake_case")]
pub struct CodeConfig {
    /// Theme to use for code blocks.
    ///
    /// This uses the [bat](https://github.com/sharkdp/bat) theme names.
    #[setting(default = "Monokai Extended")]
    pub theme: String,

    /// Whether to colorize code blocks.
    #[setting(default = true)]
    pub color: bool,

    /// Show line numbers in code blocks.
    #[setting(default = false)]
    pub line_numbers: bool,

    /// Show a link to the file containing the source code in code blocks.
    ///
    /// Can be one of: `off`, `full`, `osc8`.
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
            "" => *self = kv.try_object()?,
            "theme" => self.theme = kv.try_some_string()?,
            "color" => self.color = kv.try_some_bool()?,
            "line_numbers" => self.line_numbers = kv.try_some_bool()?,
            "file_link" => self.file_link = kv.try_some_from_str()?,
            "copy_link" => self.copy_link = kv.try_some_from_str()?,
            _ => return missing_key(&kv),
        }

        Ok(())
    }
}
