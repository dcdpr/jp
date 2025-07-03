use confique::Config as Confique;
use serde::{Deserialize, Serialize};

use super::LinkStyle;
use crate::{
    assignment::{set_error, AssignKeyValue, KvAssignment},
    error::Result,
    serde::de_from_str,
};

/// Code style configuration.
#[derive(Debug, Clone, PartialEq, Confique, Serialize, Deserialize)]
#[config(partial_attr(derive(Debug, Clone, PartialEq, Serialize)))]
#[config(partial_attr(serde(deny_unknown_fields)))]
pub struct Code {
    /// Theme to use for code blocks.
    ///
    /// This uses the [bat](https://github.com/sharkdp/bat) theme names.
    #[config(default = "Monokai Extended", env = "JP_STYLE_CODE_THEME")]
    pub theme: String,

    /// Whether to colorize code blocks.
    #[config(default = true, env = "JP_STYLE_CODE_COLOR")]
    pub color: bool,

    /// Show line numbers in code blocks.
    #[config(default = false, env = "JP_STYLE_CODE_LINE_NUMBERS")]
    pub line_numbers: bool,

    /// Show a link to the file containing the source code in code blocks.
    ///
    /// Can be one of: `off`, `full`, `osc8`.
    ///
    /// See: <https://gist.github.com/egmontkob/eb114294efbcd5adb1944c9f3cb5feda>
    #[config(default = "osc8", deserialize_with = de_from_str)]
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
    #[config(default = "off", deserialize_with = de_from_str)]
    pub copy_link: LinkStyle,
}

impl Default for Code {
    fn default() -> Self {
        Self {
            theme: "Monokai Extended".to_string(),
            color: true,
            line_numbers: false,
            file_link: LinkStyle::Osc8,
            copy_link: LinkStyle::Off,
        }
    }
}

impl AssignKeyValue for <Code as Confique>::Partial {
    fn assign(&mut self, kv: KvAssignment) -> Result<()> {
        let k = kv.key().as_str().to_owned();
        match k.as_str() {
            "theme" => self.theme = Some(kv.try_into_string()?),
            "color" => self.color = Some(kv.try_into_bool()?),
            "file_link" => self.file_link = Some(kv.try_into_string()?.parse()?),
            "copy_link" => self.copy_link = Some(kv.try_into_string()?.parse()?),

            _ => return Err(set_error(kv.key())),
        }

        Ok(())
    }
}
