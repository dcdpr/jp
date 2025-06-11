use std::time::Duration;

use confique::Config as Confique;
use serde::Deserialize as _;

use crate::{error::Result, Error};

/// Typewriter style configuration.
#[derive(Debug, Clone, PartialEq, Confique)]
pub struct Config {
    /// Delay between printing characters.
    ///
    /// You can use one of the following formats:
    /// - `10` for 10 milliseconds
    /// - `5m` for 5 milliseconds
    /// - `1u` for 1 microsecond
    /// - `0` to disable
    #[config(
        default = "3",
        env = "JP_STYLE_TYPEWRITER_TEXT_DELAY",
        deserialize_with = de_delay
    )]
    pub text_delay: Duration,

    /// Delay between printing characters.
    ///
    /// You can use one of the following formats:
    /// - `10` for 10 milliseconds
    /// - `5m` for 5 milliseconds
    /// - `1u` for 1 microsecond
    /// - `0` to disable
    #[config(
        default = "500u",
        env = "JP_STYLE_TYPEWRITER_CODE_DELAY",
        deserialize_with = de_delay
    )]
    pub code_delay: Duration,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            text_delay: Duration::from_millis(3),
            code_delay: Duration::from_micros(500),
        }
    }
}

impl Config {
    /// Set a configuration value using a stringified key/value pair.
    pub fn set(&mut self, path: &str, key: &str, value: impl Into<String>) -> Result<()> {
        let s: String = value.into();

        match key {
            "text_delay" => self.text_delay = parse_delay_config(key, &s)?,
            "code_delay" => self.code_delay = parse_delay_config(key, &s)?,
            _ => return crate::set_error(path, key),
        }

        Ok(())
    }
}

pub fn de_delay<'de, D>(deserializer: D) -> std::result::Result<Duration, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let s = String::deserialize(deserializer)?;
    parse_delay(&s).map_err(serde::de::Error::custom)
}

fn parse_delay(s: &str) -> std::result::Result<Duration, String> {
    s.rsplit_once(|c: char| c.is_ascii_digit())
        .and_then(|(s, u)| match u {
            "m" => s.parse::<u64>().map(Duration::from_millis).ok(),
            "u" => s.parse::<u64>().map(Duration::from_micros).ok(),
            _ => None,
        })
        .or_else(|| s.parse::<u64>().map(Duration::from_millis).ok())
        .ok_or(format!("invalid duration: {s}"))
}

fn parse_delay_config(k: &str, s: &str) -> std::result::Result<Duration, Error> {
    parse_delay(s).map_err(|_| Error::InvalidConfigValue {
        key: k.to_owned(),
        value: s.to_string(),
        need: vec![
            "0".to_owned(),
            "10".to_owned(),
            "5m".to_owned(),
            "1u".to_owned(),
        ],
    })
}
