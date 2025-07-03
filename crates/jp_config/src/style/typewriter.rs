use std::time::Duration;

use confique::Config as Confique;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    assignment::{set_error, AssignKeyValue, KvAssignment, KvValue},
    error::Result,
    Error,
};

/// Typewriter style configuration.
#[derive(Debug, Clone, PartialEq, Confique, Serialize, Deserialize)]
#[config(partial_attr(derive(Debug, Clone, PartialEq, Serialize)))]
#[config(partial_attr(serde(deny_unknown_fields)))]
pub struct Typewriter {
    /// Delay between printing characters.
    ///
    /// You can use one of the following formats:
    /// - `10` for 10 milliseconds
    /// - `5m` for 5 milliseconds
    /// - `1u` for 1 microsecond
    /// - `0` to disable
    #[config(
        default = "3",
        partial_attr(serde(serialize_with = "ser_delay")),
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
        partial_attr(serde(serialize_with = "ser_delay")),
        deserialize_with = de_delay
    )]
    pub code_delay: Duration,
}

impl AssignKeyValue for <Typewriter as Confique>::Partial {
    fn assign(&mut self, kv: KvAssignment) -> Result<()> {
        let k = kv.key().as_str().to_owned();
        match k.as_str() {
            "text_delay" => {
                let path = kv.key().path().to_owned();
                self.text_delay = Some(match kv.value() {
                    KvValue::Json(Value::Object(_)) => kv.try_into_object()?,
                    _ => parse_delay_config(&path, &kv.try_into_string()?)?,
                });
            }
            "code_delay" => {
                let path = kv.key().path().to_owned();
                self.code_delay = Some(match kv.value() {
                    KvValue::Json(Value::Object(_)) => kv.try_into_object()?,
                    _ => parse_delay_config(&path, &kv.try_into_string()?)?,
                });
            }

            _ => return Err(set_error(kv.key())),
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

pub fn ser_delay<S>(
    datetime: &Option<Duration>,
    serializer: S,
) -> std::result::Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    datetime
        .map(|v| format!("{}u", v.as_micros()))
        .serialize(serializer)
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
    parse_delay(s).map_err(|_| Error::InvalidConfigValueType {
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
