use std::str::FromStr;

use confique::Config as Confique;
use serde::{Deserialize, Serialize};

use crate::{
    assignment::{set_error, AssignKeyValue, KvAssignment},
    error::{Error, Result},
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

            _ => return set_error(kv.key()),
        }

        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize, Serialize)]
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
            _ => Err(Error::InvalidConfigValueType {
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
