use std::str::FromStr;

use confique::Config as Confique;
use serde::Deserialize;

use crate::error::{Error, Result};

/// Provider configuration.
#[derive(Debug, Clone, Confique)]
pub struct Config {
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
    #[config(default = "osc8", env = "JP_STYLE_CODE_FILE_LINK", deserialize_with = de_link_style)]
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
    #[config(default = "off", env = "JP_STYLE_CODE_COPY_LINK", deserialize_with = de_link_style)]
    pub copy_link: LinkStyle,
}

impl Config {
    /// Set a configuration value using a stringified key/value pair.
    pub fn set(&mut self, key: &str, value: impl Into<String>) -> Result<()> {
        match key {
            "theme" => self.theme = value.into(),
            "color" => self.color = value.into().parse()?,
            "file_link" => self.file_link = value.into().parse()?,
            "copy_link" => self.copy_link = value.into().parse()?,
            _ => return crate::set_error(key),
        }

        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LinkStyle {
    Off,
    Full,
    Osc8,
}

impl FromStr for LinkStyle {
    type Err = Error;

    fn from_str(style: &str) -> Result<Self> {
        match style {
            "off" => Ok(Self::Off),
            "full" => Ok(Self::Full),
            "osc8" => Ok(Self::Osc8),
            _ => Err(Error::InvalidConfigValue {
                key: style.to_string(),
                value: style.to_string(),
                need: vec!["off".to_owned(), "full".to_owned(), "osc8".to_owned()],
            }),
        }
    }
}

fn de_link_style<'de, D>(deserializer: D) -> std::result::Result<LinkStyle, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let string: String = String::deserialize(deserializer)?;
    LinkStyle::from_str(&string).map_err(serde::de::Error::custom)
}
