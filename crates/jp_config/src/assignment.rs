use std::str::FromStr;

use serde::{de::DeserializeOwned, Serialize};
use serde_json::Value;

use crate::{parse::try_parse_vec, Config, Error};

pub trait AssignKeyValue {
    /// Assign a value to a key in a configuration.
    fn assign(&mut self, kv: KvAssignment) -> Result<(), Error>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KvAssignment {
    /// The configuration key to set.
    pub(crate) key: KvKey,

    /// The value to set the key to.
    ///
    /// If `raw` is true, the value is used as-is. Otherwise it is parsed as a
    /// string.
    pub(crate) value: KvValue,

    /// Whether the value should be merged with the existing value, or replaced
    /// entirely.
    strategy: AssignmentStrategy,
}

impl KvAssignment {
    #[must_use]
    pub fn key(&self) -> &KvKey {
        &self.key
    }

    #[must_use]
    pub fn key_mut(&mut self) -> &mut KvKey {
        &mut self.key
    }

    #[must_use]
    pub fn value(&self) -> &KvValue {
        &self.value
    }

    #[must_use]
    pub(crate) fn is_merge(&self) -> bool {
        matches!(self.strategy, AssignmentStrategy::Merge)
    }

    /// Trim the start of the key, if it matches `segment`.
    ///
    /// See [`KvKey::trim_prefix`].
    pub(crate) fn trim_prefix(&mut self, segment: &str) -> bool {
        self.key.trim_prefix(segment)
    }

    /// Parse an assignment from an environment variable.
    ///
    /// The environment variable is expected to be in the format
    /// `KEY=[+:]VALUE`, where `KEY` is the key to set, and `VALUE` is the value
    /// to set it to.
    ///
    /// To assign a JSON value, use the `:` prefix:
    ///
    /// ```shell,ignore
    /// FOO=:{"bar":"baz"}
    /// ```
    ///
    /// To merge a value, use the `+` prefix:
    ///
    /// ```shell,ignore
    /// FOO="foo"
    /// FOO="+bar,baz"
    /// ```
    pub(crate) fn try_from_env(key: impl Into<String>, value: &str) -> Result<Self, Error> {
        // Are we parsing the prefix?
        let mut prefix = true;
        // Is the value raw JSON?
        let mut raw = false;
        // Does the value need to be merged?
        let mut merge = false;

        let mut chars = value.chars().peekable();
        let mut value = String::new();
        while let Some(c) = chars.next() {
            if prefix && c == '\\' && chars.peek() == Some(&':') {
                prefix = false;
                chars.next();
                value.push(':');
            } else if prefix && c == '\\' && chars.peek() == Some(&'+') {
                prefix = false;
                chars.next();
                value.push('+');
            } else if prefix && !raw && c == ':' {
                raw = true;
                value.push(c);
            } else if prefix && !merge && c == '+' {
                merge = true;
                value.push(c);
            } else {
                prefix = false;
                value.push(c);
            }
        }

        let key: String = key.into();
        Ok(Self {
            key: KvKey {
                path: key.clone(),
                delim: KeyDelim::Underscore,
                full_path: key,
            },
            value: if raw {
                KvValue::Json(serde_json::from_str(&value)?)
            } else {
                KvValue::String(value.to_string())
            },
            strategy: if merge {
                AssignmentStrategy::Merge
            } else {
                AssignmentStrategy::Set
            },
        })
    }

    /// Parse an assignment from a CLI argument.
    ///
    /// The argument is expected to be in the format `KEY[+:]=VALUE`, where
    /// `KEY` is the key to set, and `VALUE` is the value to set it to.
    ///
    /// To assign a JSON value, use the `:` prefix:
    ///
    /// ```shell,ignore
    /// $ jp --cfg 'foo:={"bar":"baz"}'
    /// ```
    ///
    /// To merge a value, use the `+` prefix:
    ///
    /// ```shell,ignore
    /// $ jp --cfg 'foo=foo'
    /// $ jp --cfg 'foo+=bar,baz'
    /// ```
    pub(crate) fn try_from_cli(key: &str, value: &str) -> Result<Self, Error> {
        // Is the value raw JSON?
        let mut raw = false;
        // Does the value need to be merged?
        let mut merge = false;

        let mut key = key
            .chars()
            .rev()
            .skip_while(|&c| {
                if !raw && c == ':' {
                    raw = true;
                    true
                } else if !merge && c == '+' {
                    merge = true;
                    true
                } else {
                    false
                }
            })
            .collect::<Vec<_>>();

        key.reverse();

        let key: String = key.into_iter().collect();
        Ok(Self {
            key: KvKey {
                path: key.clone(),
                delim: KeyDelim::Dot,
                full_path: key,
            },
            value: if raw {
                KvValue::Json(serde_json::from_str(value)?)
            } else {
                KvValue::String(value.to_string())
            },
            strategy: if merge {
                AssignmentStrategy::Merge
            } else {
                AssignmentStrategy::Set
            },
        })
    }

    pub(crate) fn try_into_object<T: DeserializeOwned>(self) -> Result<T, Error> {
        let Self { key, value, .. } = self;

        match value {
            KvValue::Json(v @ Value::Object(_)) => Ok(serde_json::from_value(v.clone())?),
            v => Err(Error::InvalidConfigValueType {
                key: key.full_path,
                value: serde_json::to_string(&v)?,
                need: vec!["object".to_owned()],
            }),
        }
    }

    pub(crate) fn into_value(self) -> Value {
        let Self { value, .. } = self;
        match value {
            KvValue::Json(v) => v,
            KvValue::String(v) => Value::String(v),
        }
    }

    pub(crate) fn try_into_string(self) -> Result<String, Error> {
        let Self { key, value, .. } = self;

        match value {
            KvValue::Json(Value::String(v)) | KvValue::String(v) => Ok(v),
            KvValue::Json(_) => Err(Error::InvalidConfigValueType {
                key: key.full_path,
                value: serde_json::to_string(&value)?,
                need: vec!["string".to_owned()],
            }),
        }
    }

    pub(crate) fn try_into_bool(self) -> Result<bool, Error> {
        let Self { key, value, .. } = self;

        match value {
            KvValue::Json(Value::Bool(v)) => Ok(v),
            KvValue::String(v) => Ok(v.parse()?),
            v @ KvValue::Json(_) => Err(Error::InvalidConfigValueType {
                key: key.full_path,
                value: serde_json::to_string(&v)?,
                need: vec!["bool".to_owned(), "string".to_owned()],
            }),
        }
    }

    #[expect(dead_code)]
    pub(crate) fn try_into_u32(self) -> Result<u32, Error> {
        let Self { key, value, .. } = self;

        match value {
            #[expect(clippy::cast_possible_truncation)]
            KvValue::Json(Value::Number(v)) if v.is_u64() => Ok(v.as_u64().unwrap() as u32),
            KvValue::String(v) => Ok(v.parse()?),
            v @ KvValue::Json(_) => Err(Error::InvalidConfigValueType {
                key: key.full_path,
                value: serde_json::to_string(&v)?,
                need: vec!["bool".to_owned(), "string".to_owned()],
            }),
        }
    }

    pub(crate) fn try_set_or_merge_vec<T>(
        self,
        vec: &mut Vec<T>,
        parser: impl Fn(Value) -> Result<T, Box<dyn std::error::Error + Send + Sync>>,
    ) -> Result<(), Error> {
        let merge = self.is_merge();
        let Self { key, value, .. } = self;

        let v = match value {
            KvValue::Json(Value::Array(v)) => v
                .into_iter()
                .map(|v| {
                    parser(v.clone()).or_else(|error| {
                        Err(Error::ValueParseError {
                            key: key.full_path.clone(),
                            value: serde_json::to_string(&v)?,
                            error: error.to_string(),
                        })
                    })
                })
                .collect::<Result<Vec<_>, _>>()?,
            KvValue::Json(Value::String(s)) | KvValue::String(s) => try_parse_vec(&s, |s| {
                parser(Value::String(s.to_owned())).map_err(|error| Error::ValueParseError {
                    key: key.full_path.clone(),
                    value: s.to_string(),
                    error: error.to_string(),
                })
            })?,
            v @ KvValue::Json(_) => {
                return Err(Error::InvalidConfigValueType {
                    key: key.full_path.clone(),
                    value: serde_json::to_string(&v)?,
                    need: vec!["string".to_owned(), "array".to_owned()],
                })
            }
        };

        if merge {
            vec.extend(v);
        } else {
            *vec = v;
        }

        Ok(())
    }
}

impl FromStr for KvAssignment {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Error> {
        s.split_once('=')
            .ok_or(Error::InvalidConfigValueType {
                key: s.to_string(),
                value: s.to_string(),
                need: vec!["<key>[:+]=<value>".to_string(), "@<path>".to_string()],
            })
            .and_then(|(key, value)| Self::try_from_cli(key, value))
    }
}

/// The strategy to use for setting a value in a configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AssignmentStrategy {
    /// The value should be merged with the existing value, if applicable.
    Merge,

    /// The value should be set, overwriting any existing value.
    Set,
}

/// A key in a configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KvKey {
    /// The "path" of the key.
    path: String,

    /// The delimiter used to separate the path elements.
    delim: KeyDelim,

    /// The full path, unchanged even after calling `trim_prefix`.
    full_path: String,
}

impl KvKey {
    /// The (possibly trimmed) key value.
    pub(crate) fn as_str(&self) -> &str {
        &self.path
    }

    /// The full path, unchanged even after calling `trim_prefix`.
    #[expect(clippy::misnamed_getters)]
    pub(crate) fn path(&self) -> &str {
        &self.full_path
    }

    pub(crate) fn segments(&self) -> impl Iterator<Item = &str> {
        self.path.split(self.delim.as_str())
    }

    /// Trim the start of the key for the nunmber of characters specified.
    ///
    /// For example, given the key `foo.bar.baz`, calling `trim_prefix("foo")`
    /// will result in `bar.baz`.
    ///
    /// Returns `true` if the key was trimmed.
    pub(crate) fn trim_prefix(&mut self, segment: &str) -> bool {
        let mut segments = self.segments();
        if segments.next() != Some(segment) {
            return false;
        }

        self.path = segments.collect::<Vec<_>>().join(self.delim.as_str());
        true
    }

    /// Trim the first segment of the key, returning it.
    pub(crate) fn trim_any_prefix(&mut self) -> Option<String> {
        let segment = self.segments().next()?.to_owned();
        self.trim_prefix(&segment);
        Some(segment)
    }
}

/// The delimiter used to separate the path elements.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KeyDelim {
    /// Dot-delimited key, e.g. `foo.bar`.
    Dot,

    /// Underscore-delimited key, e.g. `foo_bar`.
    Underscore,
}

impl KeyDelim {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Dot => ".",
            Self::Underscore => "_",
        }
    }
}

/// A value to set in a configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum KvValue {
    /// A raw JSON value.
    Json(Value),

    /// A string value.
    String(String),
}

pub(crate) fn set_error(k: &KvKey) -> Error {
    Error::UnknownConfigKey {
        key: k.path().to_owned(),
        available_keys: {
            let mut keys = Config::fields();
            let mut path = Some(k.path());
            while let Some(prefix) = path {
                path = prefix.rsplit_once('.').map(|(prefix, _)| prefix);

                let matches = Config::fields()
                    .into_iter()
                    .filter(|f| f.starts_with(prefix))
                    .collect::<Vec<_>>();

                if !matches.is_empty() {
                    keys = matches;
                    break;
                }
            }

            keys
        },
    }
}
