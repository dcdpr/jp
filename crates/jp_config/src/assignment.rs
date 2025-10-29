//! Features for assigning key-value pairs to configurations.
//!
//! It uses the [`KvAssignment`] type to parse key-value pairs from CLI
//! arguments or environment variables. The [`AssignKeyValue`] trait is
//! implemented for types that can be assigned a key-value pair.

use std::{fmt, str::FromStr};

use schematic::PartialConfig;
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, from_str};

use crate::{AppConfig, BoxedError};

/// The result of assigning a key-value pair to a configuration.
pub type AssignResult = Result<(), BoxedError>;

/// A trait for assigning a key-value pair to a configuration.
pub trait AssignKeyValue {
    /// Assign a value to a key in a configuration.
    ///
    /// # Errors
    ///
    /// Can return any error that implements [`std::error::Error`], when
    /// assignment fails (e.g. type error, parse error, unknown key, etc.).
    fn assign(&mut self, kv: KvAssignment) -> AssignResult;
}

impl<T> AssignKeyValue for Option<T>
where
    T: AssignKeyValue + Default,
{
    fn assign(&mut self, kv: KvAssignment) -> Result<(), BoxedError> {
        self.get_or_insert_default().assign(kv)
    }
}

/// A key-value pair to set in a configuration.
///
/// The key is a path-like string, separated by a configurable delimiter. The
/// value is a string, or a JSON object, and the strategy determines how the
/// value is set.
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
    strategy: Strategy,
}

impl FromStr for KvAssignment {
    type Err = KvAssignmentError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        s.split_once('=')
            .ok_or(KvAssignmentError::new(s, KvAssignmentErrorKind::KvParse {
                kv: s.to_string(),
                expected: &[
                    "<key>=<value> (assignment)",
                    "<key>:=<value> (json assignment)",
                    "<key>+=<value> (merge assignment)",
                    "<key>:+=<value> (json merge assignment)",
                ],
            }))
            .and_then(|(key, value)| Self::try_from_cli(key, value))
    }
}

/// An error that occurred while assigning a key-value pair.
#[derive(Debug, thiserror::Error)]
pub struct KvAssignmentError {
    /// The full path of the key that failed to parse.
    pub key: String,

    /// The underlying error.
    #[source]
    pub error: KvAssignmentErrorKind,
}

impl KvAssignmentError {
    /// Create a new error with the given key and error.
    pub fn new<E>(key: impl Into<String>, error: E) -> Self
    where
        E: Into<KvAssignmentErrorKind>,
    {
        Self {
            key: key.into(),
            error: error.into(),
        }
    }
}

impl fmt::Display for KvAssignmentError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.key, self.error)
    }
}

/// The underlying error type for a key-value assignment.
#[derive(Debug, thiserror::Error)]
pub enum KvAssignmentErrorKind {
    /// Key-value pair parse error.
    #[error("unable to parse key-value pair")]
    KvParse {
        /// The key-value pair.
        kv: String,

        /// The valid formats.
        expected: &'static [&'static str],
    },

    /// A type error occurred.
    #[error("type error")]
    Type {
        /// The value that failed to parse.
        value: String,
        /// The types that were expected.
        need: Vec<String>,
    },

    /// A parse error occurred.
    #[error("parse error")]
    Parse {
        /// The value that failed to parse.
        value: Value,

        /// The underlying parse error.
        #[source]
        error: BoxedError,
    },

    /// An unknown key was encountered.
    #[error("unknown key")]
    UnknownKey {
        /// The valid keys.
        known_keys: Vec<String>,
    },

    /// An out of bounds index was encountered.
    #[error("unknown index")]
    UnknownIndex {
        /// The index.
        index: usize,

        /// The number of elements in the array.
        elements_count: usize,
    },

    /// A JSON error occurred.
    #[error(transparent)]
    Json(#[from] serde_json::Error),

    /// A boolean parse error occurred.
    #[error(transparent)]
    ParseBool(#[from] std::str::ParseBoolError),

    /// An integer parse error occurred.
    #[error(transparent)]
    ParseInt(#[from] std::num::ParseIntError),

    /// A float parse error occurred.
    #[error(transparent)]
    ParseFloat(#[from] std::num::ParseFloatError),
}

impl KvAssignment {
    /// The [`KvKey`] of the assignment.
    #[must_use]
    pub(crate) const fn key(&self) -> &KvKey {
        &self.key
    }

    /// The [`KvKey`] of the assignment, as a string.
    #[must_use]
    pub(crate) fn key_string(&self) -> String {
        self.key.path.clone()
    }

    /// Whether the assignment should be merged with the existing value, or
    /// replace it.
    #[must_use]
    pub(crate) const fn is_merge(&self) -> bool {
        matches!(self.strategy, Strategy::Merge)
    }

    /// Trim the start of the key, if it matches `segment`.
    ///
    /// See [`KvKey::trim_prefix`].
    pub(crate) fn trim_prefix(&mut self, segment: &str) -> bool {
        self.key.trim_prefix(segment)
    }

    /// Trim the first segment of the key, returning it.
    ///
    /// See [`KvKey::trim_prefix_any`].
    pub(crate) fn trim_prefix_any(&mut self) -> Option<String> {
        self.key.trim_prefix_any()
    }

    /// Convenience method for [`Self::trim_prefix`].
    pub(crate) fn p(&mut self, segment: &str) -> bool {
        self.trim_prefix(segment)
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
    pub(crate) fn try_from_env(
        key: impl Into<String>,
        value: &str,
    ) -> Result<Self, KvAssignmentError> {
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
            } else if prefix && !merge && c == '+' {
                merge = true;
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
                full_path: key.clone(),
            },
            value: if raw {
                let json = from_str(&value).map_err(|err| KvAssignmentError::new(key, err))?;
                KvValue::Json(json)
            } else {
                KvValue::String(value.clone())
            },
            strategy: if merge {
                Strategy::Merge
            } else {
                Strategy::Set
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
    pub(super) fn try_from_cli(
        key: impl AsRef<str>,
        value: &str,
    ) -> Result<Self, KvAssignmentError> {
        // Is the value raw JSON?
        let mut raw = false;
        // Does the value need to be merged?
        let mut merge = false;

        let mut key = key
            .as_ref()
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
        let key = KvKey {
            path: key.clone(),
            delim: KeyDelim::Dot,
            full_path: key,
        };
        Ok(Self {
            key: key.clone(),
            value: if raw {
                KvValue::Json(serde_json::from_str(value).map_err(|err| kv_error(&key, err))?)
            } else {
                KvValue::String(value.to_string())
            },
            strategy: if merge {
                Strategy::Merge
            } else {
                Strategy::Set
            },
        })
    }

    /// Try to parse the value as a JSON object.
    pub(crate) fn try_object<T: DeserializeOwned>(self) -> Result<T, KvAssignmentError> {
        let Self { key, value, .. } = self;

        match value {
            KvValue::Json(v @ Value::Object(_)) => {
                serde_json::from_value(v).map_err(|err| kv_error(&key, err))
            }
            _ => type_error(&key, &value, &["object"]),
        }
    }

    /// Try to parse the value as a JSON object or use [`FromStr`].
    pub(crate) fn try_object_or_from_str<T, E>(self) -> Result<T, KvAssignmentError>
    where
        T: DeserializeOwned + FromStr<Err = E>,
        E: Into<BoxedError>,
    {
        let Self { key, value, .. } = self;

        match value {
            KvValue::Json(v @ Value::Object(_)) => {
                serde_json::from_value(v).map_err(|err| kv_error(&key, err))
            }
            KvValue::Json(Value::String(s)) | KvValue::String(s) => T::from_str(&s)
                .map_err(Into::into)
                .or_else(|err| assignment_error(&key, Value::String(s), err)),

            KvValue::Json(_) => type_error(&key, &value, &["object", "string"]),
        }
    }

    /// Convenience method for [`Self::try_object_or_from_str`] that wraps the `Ok` value
    /// into `Some`.
    pub(crate) fn try_some_object_or_from_str<T, E>(self) -> Result<Option<T>, KvAssignmentError>
    where
        T: DeserializeOwned + FromStr<Err = E>,
        E: Into<BoxedError>,
    {
        self.try_object_or_from_str().map(Some)
    }

    /// Try to parse the value using [`FromStr`].
    pub(crate) fn try_from_str<T, E>(self) -> Result<T, KvAssignmentError>
    where
        T: FromStr<Err = E>,
        E: Into<BoxedError>,
    {
        let Self { key, value, .. } = self;

        match value {
            KvValue::Json(Value::String(s)) | KvValue::String(s) => T::from_str(&s)
                .map_err(Into::into)
                .or_else(|err| assignment_error(&key, Value::String(s), err)),
            KvValue::Json(_) => type_error(&key, &value, &["string"]),
        }
    }

    /// Convenience method for [`Self::try_from_str`] that wraps the `Ok` value
    /// into `Some`.
    pub(crate) fn try_some_from_str<T, E>(self) -> Result<Option<T>, KvAssignmentError>
    where
        T: FromStr<Err = E>,
        E: Into<BoxedError>,
    {
        self.try_from_str().map(Some)
    }

    /// Try to parse the value as a string.
    pub(crate) fn try_string(self) -> Result<String, KvAssignmentError> {
        let Self { key, value, .. } = self;

        match value {
            KvValue::Json(Value::String(v)) | KvValue::String(v) => Ok(v),
            KvValue::Json(_) => type_error(&key, &value, &["string"]),
        }
    }

    /// Convenience method for [`Self::try_string`] that wraps the `Ok` value
    /// into `Some`.
    pub(crate) fn try_some_string(self) -> Result<Option<String>, KvAssignmentError> {
        self.try_string().map(Some)
    }

    /// Try to parse the value as a boolean.
    pub(crate) fn try_bool(self) -> Result<bool, KvAssignmentError> {
        let Self { key, value, .. } = self;

        match value {
            KvValue::Json(Value::Bool(v)) => Ok(v),
            KvValue::Json(_) => type_error(&key, &value, &["bool", "string"]),
            KvValue::String(v) => Ok(v
                .parse()
                .map_err(|err| KvAssignmentError::new(key.full_path.clone(), err))?),
        }
    }

    /// Convenience method for [`Self::try_bool`] that wraps the `Ok` value into
    /// `Some`.
    pub(crate) fn try_some_bool(self) -> Result<Option<bool>, KvAssignmentError> {
        self.try_bool().map(Some)
    }

    /// Try to parse the value as an unsigned 32-bit integer.
    pub(crate) fn try_u32(self) -> Result<u32, KvAssignmentError> {
        let Self { key, value, .. } = self;

        match value {
            #[expect(clippy::cast_possible_truncation)]
            KvValue::Json(Value::Number(v)) if v.is_u64() => Ok(v.as_u64().expect("is u64") as u32),
            KvValue::Json(_) => type_error(&key, &value, &["number", "string"]),
            KvValue::String(v) => Ok(v
                .parse()
                .map_err(|err| KvAssignmentError::new(key.full_path.clone(), err))?),
        }
    }

    /// Convenience method for [`Self::try_u32`] that wraps the `Ok` value into
    /// `Some`.
    pub(crate) fn try_some_u32(self) -> Result<Option<u32>, KvAssignmentError> {
        self.try_u32().map(Some)
    }

    /// Try to parse the value as a 32-bit floating point number.
    pub(crate) fn try_f32(self) -> Result<f32, KvAssignmentError> {
        let Self { key, value, .. } = self;

        match value {
            #[expect(clippy::cast_possible_truncation)]
            KvValue::Json(Value::Number(v)) if v.is_f64() => Ok(v.as_f64().expect("is f64") as f32),
            KvValue::Json(_) => type_error(&key, &value, &["float", "string"]),
            KvValue::String(v) => Ok(v
                .parse()
                .map_err(|err| KvAssignmentError::new(key.full_path.clone(), err))?),
        }
    }

    /// Convenience method for [`Self::try_f32`] that wraps the `Ok` value into
    /// `Some`.
    pub(crate) fn try_some_f32(self) -> Result<Option<f32>, KvAssignmentError> {
        self.try_f32().map(Some)
    }

    /// Try to parse the value as a JSON array of partial configs, and set or
    /// merge the elements.
    pub(crate) fn try_vec<T>(
        mut self,
        vec: &mut Vec<T>,
        parser: impl Fn(Self) -> Result<T, BoxedError>,
    ) -> Result<(), KvAssignmentError> {
        // If the key is an index into the array, assign the value to the
        // element, if it exists.
        if let Some(i) = self.key.trim_index() {
            match vec.get_mut(i) {
                Some(item) => {
                    let mut kv = self.clone();
                    let k = if kv.key.is_empty() {
                        i.to_string()
                    } else {
                        format!("{i}{}", kv.key.delim.as_str())
                    };
                    kv.key.path.insert_str(0, &k);

                    *item = parser(self.clone())
                        .or_else(|err| assignment_error(&self.key, self.value.into_value(), err))?;

                    return Ok(());
                }
                None => return vec_missing_index_error(&self.key, i, vec.len()),
            }
        }

        // The next key segment must either be an index into the array (handled
        // above), or no segment at all (handled below). A regular segment means
        // the key expects a JSON object, which is not an array.
        if !self.key.is_empty() {
            return type_error(&self.key, &self.value, &["array"]);
        }

        let merge = self.is_merge();
        let v = match self.value.clone() {
            KvValue::Json(Value::Array(v)) => v
                .into_iter()
                .enumerate()
                .map(|(i, v)| {
                    let mut kv = self.clone();
                    kv.key.path = i.to_string();
                    kv.value = KvValue::Json(v.clone());

                    parser(kv).or_else(|err| assignment_error(&self.key, v, err))
                })
                .collect::<Result<Vec<_>, _>>()?,
            KvValue::Json(Value::String(s)) | KvValue::String(s) => {
                try_parse_vec(&self.key, &s, |i, s| {
                    let mut kv = self.clone();
                    kv.key.path = i.to_string();
                    kv.value = KvValue::String(s.into());

                    parser(kv).or_else(|err| {
                        assignment_error(&self.key, Value::String(s.to_owned()), err)
                    })
                })?
            }
            KvValue::Json(_) => type_error(&self.key, &self.value, &["string", "array"])?,
        };

        if merge {
            vec.extend(v);
        } else {
            *vec = v;
        }

        Ok(())
    }

    /// Specialized version of [`Self::try_vec`] for parsing a JSON array of
    /// strings.
    pub(crate) fn try_vec_of_strings<T>(self, vec: &mut Vec<T>) -> Result<(), KvAssignmentError>
    where
        T: From<String>,
    {
        let parser = |kv: Self| match kv.value.clone().into_value() {
            Value::String(v) => Ok(v.into()),
            _ => type_error(kv.key(), &kv.value, &["string"]).map_err(Into::into),
        };

        self.try_vec(vec, parser)
    }

    /// Convenience method for [`Self::try_vec_of_strings`] that wraps the `Ok`
    /// value into
    pub(crate) fn try_some_vec_of_strings<T>(
        self,
        vec: &mut Option<Vec<T>>,
    ) -> Result<(), KvAssignmentError>
    where
        T: From<String>,
    {
        self.try_vec_of_strings(vec.get_or_insert_default())
    }

    /// Try to parse the value as a JSON array of partial configs, and set or
    /// merge the elements.
    pub(crate) fn try_vec_of_nested<T>(mut self, vec: &mut Vec<T>) -> Result<(), KvAssignmentError>
    where
        T: PartialConfig + AssignKeyValue + FromStr<Err = BoxedError>,
    {
        // If the key is an index into the array, assign the value to the
        // element, if it exists.
        if let Some(i) = self.key.trim_index() {
            match vec.get_mut(i) {
                // If we have an index into the array, and no more key elements
                // follow, we need to replace the value at the index with the
                // value from the assignment.
                Some(item) if self.key.is_empty() => {
                    match self.value.clone() {
                        KvValue::Json(v @ Value::Object(_)) => {
                            for (k, v) in flatten_json_object(v, self.key.delim) {
                                let mut kv = self.clone();
                                kv.key.full_path = [self.key.full_path.as_str(), k.as_str()]
                                    .join(kv.key.delim.as_str());
                                kv.key.path = k;
                                kv.value = KvValue::Json(v.clone());

                                item.assign(kv)
                                    .or_else(|err| assignment_error(&self.key, v, err))?;
                            }
                        }
                        KvValue::Json(Value::String(s)) | KvValue::String(s) => {
                            *item = T::from_str(&s).or_else(|err| {
                                assignment_error(&self.key, Value::String(s), err)
                            })?;
                        }
                        KvValue::Json(_) => {
                            return type_error(&self.key, &self.value, &["string", "object"])?;
                        }
                    }

                    return Ok(());
                }
                // If we have an index into the array, and more key elements
                // follow, we delegate the assignment to the value at the index.
                Some(v) => {
                    return v
                        .assign(self.clone())
                        .or_else(|err| assignment_error(&self.key, self.value.into_value(), err));
                }
                None => return vec_missing_index_error(&self.key, i, vec.len()),
            }
        }

        // Use try_vec for the main logic
        self.try_vec(vec, |kv| -> Result<T, BoxedError> {
            match kv.value.clone() {
                KvValue::Json(Value::Object(obj)) => {
                    let mut item = T::default();
                    for (k, v) in flatten_json_object(Value::Object(obj), kv.key.delim) {
                        let mut nested_kv = kv.clone();
                        nested_kv.key.full_path =
                            [kv.key.full_path.as_str(), k.as_str()].join(kv.key.delim.as_str());
                        nested_kv.key.path = k;
                        nested_kv.value = KvValue::Json(v.clone());

                        item.assign(nested_kv)
                            .or_else(|err| assignment_error(&kv.key, v, err))?;
                    }
                    Ok(item)
                }
                KvValue::Json(Value::String(s)) | KvValue::String(s) => T::from_str(&s),
                KvValue::Json(_) => {
                    type_error(&kv.key, &kv.value, &["string", "object"]).map_err(Into::into)
                }
            }
        })
    }

    /// Convenience method for [`Self::try_vec_of_nested`] that wraps
    /// the `Ok` value into `Some`.
    #[allow(clippy::allow_attributes, dead_code)]
    pub(crate) fn try_some_vec_of_nested<T>(
        self,
        vec: &mut Option<Vec<T>>,
    ) -> Result<(), KvAssignmentError>
    where
        T: PartialConfig + AssignKeyValue + FromStr<Err = BoxedError>,
    {
        self.try_vec_of_nested(vec.get_or_insert_default())
    }
}

/// Flatten a JSON object into a list of dot-delimited key-value pairs.
///
/// This *DOES NOT* flatten arrays, only objects. The reason for this is that if
/// we want to assign a JSON array to an item, if we flattened the array, the
/// assignment would try to fetch the individual elements from the array, which
/// might not exist, and result in an out-of-bounds error.
///
/// Instead, we want the array to be assigned as-is, overriding any existing
/// array.
///
/// Modifying individual elements of an array can still be done, by specifically
/// targeting the array index, e.g. `foo.0` or `foo.0.bar`.
fn flatten_json_object(value: Value, delim: KeyDelim) -> Vec<(String, Value)> {
    let mut result = Vec::new();
    flatten_recursive(value, String::new(), &mut result, delim);
    result
}

/// Recursively flatten a JSON object into a list of dot-delimited key-value
fn flatten_recursive(
    value: Value,
    path: String,
    result: &mut Vec<(String, Value)>,
    delim: KeyDelim,
) {
    match value {
        Value::Object(v) => {
            for (k, v) in v {
                flatten_recursive(
                    v,
                    if path.is_empty() {
                        k.clone()
                    } else {
                        format!("{path}{}{k}", delim.as_str())
                    },
                    result,
                    delim,
                );
            }
        }
        _ => result.push((path, value)),
    }
}

/// Try to parse a comma-separated list of values, using the given parser.
fn try_parse_vec<'a, T, E>(
    key: &KvKey,
    s: &'a str,
    parser: impl Fn(usize, &'a str) -> std::result::Result<T, E>,
) -> std::result::Result<Vec<T>, KvAssignmentError>
where
    E: Into<BoxedError>,
{
    s.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .enumerate()
        .map(|(i, s)| {
            parser(i, s)
                .or_else(|err| assignment_error(key, Value::String(s.to_owned()), err.into()))
        })
        .collect::<std::result::Result<Vec<_>, _>>()
}

/// Create a [`KvAssignmentError`] error from a boxed error.
fn assignment_error<T>(
    key: &KvKey,
    value: Value,
    error: BoxedError,
) -> Result<T, KvAssignmentError> {
    match error.downcast::<KvAssignmentError>() {
        Ok(error) => Err(*error),
        Err(error) => Err(KvAssignmentError::new(
            key.full_path.clone(),
            KvAssignmentErrorKind::Parse { value, error },
        )),
    }
}

/// Create a [`KvAssignmentError`] JSON error.
fn kv_error(key: &KvKey, err: serde_json::Error) -> KvAssignmentError {
    KvAssignmentError::new(key.full_path.clone(), KvAssignmentErrorKind::Json(err))
}

/// Create a [`KvAssignmentError`] type error.
pub(crate) fn type_error<T>(
    key: &KvKey,
    value: &KvValue,
    need: &[&'static str],
) -> Result<T, KvAssignmentError> {
    let value = serde_json::to_string(&value)
        .map_err(|err| KvAssignmentError::new(key.full_path.clone(), err))?;

    Err(KvAssignmentError::new(
        key.full_path.clone(),
        KvAssignmentErrorKind::Type {
            value,
            need: need.iter().map(ToString::to_string).collect(),
        },
    ))
}

/// Create a [`KvAssignmentError`] parse error.
fn vec_missing_index_error<T>(
    key: &KvKey,
    index: usize,
    elements_count: usize,
) -> Result<T, KvAssignmentError> {
    Err(KvAssignmentError::new(
        key.full_path.clone(),
        KvAssignmentErrorKind::UnknownIndex {
            index,
            elements_count,
        },
    ))
}

/// The strategy to use for setting a value in a configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Strategy {
    /// The value should be merged with the existing value, if applicable.
    Merge,

    /// The value should be set, overwriting any existing value.
    Set,
}

/// A key in a configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct KvKey {
    /// The "path" of the key.
    ///
    /// We cannot split the path by its delimiter before hand, because we don't
    /// know if the delimiter is part of the segment or not. For example, for
    /// the path `foo_bar_baz`, it might be split as `["foo", "bar", "baz"]`, or
    /// `["foo_bar", "baz"]`, or `["foo", "bar_baz"]` or `["foo_bar_baz"]`.
    ///
    /// Only once the key is used in [`AssignKeyValue::assign`], we know the
    /// shape of the next segment, and can split the path by the delimiter.
    path: String,

    /// The delimiter used to separate the path elements.
    delim: KeyDelim,

    /// The original path when the [`KvKey`] type was initialized, unchanged
    /// even if `path` is mutated through e.g. [`KvKey::trim_prefix`].
    full_path: String,
}

impl AsRef<str> for KvKey {
    fn as_ref(&self) -> &str {
        self.path.as_ref()
    }
}

impl KvKey {
    /// Whether the key is empty.
    pub(crate) const fn is_empty(&self) -> bool {
        self.path.is_empty()
    }

    /// Trim the segment from the start of the key.
    ///
    /// For example, given the key `foo.bar.baz`, calling `trim_prefix("foo")`
    /// will result in `bar.baz`.
    ///
    /// The segment is only trimmed if it is delimited by the key delimiter at
    /// the end (in which case, the delimiter is also trimmed), or if it's the
    /// last segment of the key.
    ///
    /// Returns `true` if the key was trimmed.
    pub(crate) fn trim_prefix(&mut self, segment: &str) -> bool {
        let delimited_segment = format!("{segment}{}", self.delim.as_str());

        if !self.path.starts_with(&delimited_segment) && self.path != segment {
            return false;
        }

        let mut len = segment.len();
        if self.path.len() > len {
            len += 1;
        }

        self.path = self.path[len..].to_owned();
        true
    }

    /// Similar to [`KvKey::trim_prefix`], but trims _any_ prefix, if one
    /// exists.
    pub(crate) fn trim_prefix_any(&mut self) -> Option<String> {
        let mut segments = self.path.split(self.delim.as_str());
        if let Some(key) = segments.next().map(str::to_owned) {
            self.path = segments.collect::<Vec<_>>().join(self.delim.as_str());
            return Some(key);
        }

        None
    }

    /// Similar to [`KvKey::trim_prefix`], but only trims the first segment if
    /// it is an integer, pointing into a list. Returns the index if it was
    /// trimmed, or `None` if it was not.
    pub(crate) fn trim_index(&mut self) -> Option<usize> {
        let mut segments = self.path.split(self.delim.as_str());
        if let Some(index) = segments.next().and_then(|v| v.parse().ok()) {
            self.path = segments.collect::<Vec<_>>().join(self.delim.as_str());
            return Some(index);
        }

        None
    }
}

/// The delimiter used to separate the path elements.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KeyDelim {
    /// Dot-delimited key, e.g. `foo.bar`.
    Dot,

    /// Underscore-delimited key, e.g. `foo_bar`.
    Underscore,
}

impl KeyDelim {
    /// Return the delimiter as a static string.
    const fn as_str(self) -> &'static str {
        match self {
            Self::Dot => ".",
            Self::Underscore => "_",
        }
    }
}

/// A value to set in a configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub(crate) enum KvValue {
    /// A raw JSON value.
    Json(Value),

    /// A string value.
    String(String),
}

impl KvValue {
    /// Convert the value into a JSON [`Value`].
    pub(crate) fn into_value(self) -> Value {
        match self {
            Self::Json(v) => v,
            Self::String(v) => Value::String(v),
        }
    }
}

/// Convenience method to create a missing key error.
pub(crate) fn missing_key<T>(kv: &KvAssignment) -> Result<T, BoxedError> {
    Err(KvAssignmentError::new(
        kv.key.full_path.clone(),
        KvAssignmentErrorKind::UnknownKey {
            known_keys: {
                let mut keys = AppConfig::fields();
                let mut path = Some(kv.key.full_path.as_str());
                while let Some(prefix) = path {
                    path = prefix.rsplit_once('.').map(|(prefix, _)| prefix);

                    let matches = AppConfig::fields()
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
        },
    )
    .into())
}

#[cfg(test)]
mod tests {
    use assert_matches::assert_matches;

    use super::*;

    #[test]
    fn test_kv_assignment_from_str() {
        let cases = vec![
            ("foo=bar", KvAssignment {
                key: KvKey {
                    path: "foo".to_owned(),
                    delim: KeyDelim::Dot,
                    full_path: "foo".to_owned(),
                },
                value: KvValue::String("bar".to_owned()),
                strategy: Strategy::Set,
            }),
            ("foo+=bar", KvAssignment {
                key: KvKey {
                    path: "foo".to_owned(),
                    delim: KeyDelim::Dot,
                    full_path: "foo".to_owned(),
                },
                value: KvValue::String("bar".to_owned()),
                strategy: Strategy::Merge,
            }),
            ("foo.bar=baz", KvAssignment {
                key: KvKey {
                    path: "foo.bar".to_owned(),
                    delim: KeyDelim::Dot,
                    full_path: "foo.bar".to_owned(),
                },
                value: KvValue::String("baz".to_owned()),
                strategy: Strategy::Set,
            }),
            ("foo.bar+=baz", KvAssignment {
                key: KvKey {
                    path: "foo.bar".to_owned(),
                    delim: KeyDelim::Dot,
                    full_path: "foo.bar".to_owned(),
                },
                value: KvValue::String("baz".to_owned()),
                strategy: Strategy::Merge,
            }),
            ("foo.bar.1=qux", KvAssignment {
                key: KvKey {
                    path: "foo.bar.1".to_owned(),
                    delim: KeyDelim::Dot,
                    full_path: "foo.bar.1".to_owned(),
                },
                value: KvValue::String("qux".to_owned()),
                strategy: Strategy::Set,
            }),
            ("foo.bar.1+=qux", KvAssignment {
                key: KvKey {
                    path: "foo.bar.1".to_owned(),
                    delim: KeyDelim::Dot,
                    full_path: "foo.bar.1".to_owned(),
                },
                value: KvValue::String("qux".to_owned()),
                strategy: Strategy::Merge,
            }),
            ("foo:=true", KvAssignment {
                key: KvKey {
                    path: "foo".to_owned(),
                    delim: KeyDelim::Dot,
                    full_path: "foo".to_owned(),
                },
                value: KvValue::Json(true.into()),
                strategy: Strategy::Set,
            }),
            ("foo:=42", KvAssignment {
                key: KvKey {
                    path: "foo".to_owned(),
                    delim: KeyDelim::Dot,
                    full_path: "foo".to_owned(),
                },
                value: KvValue::Json(42.into()),
                strategy: Strategy::Set,
            }),
            (r#"foo+:=["bar"]"#, KvAssignment {
                key: KvKey {
                    path: "foo".to_owned(),
                    delim: KeyDelim::Dot,
                    full_path: "foo".to_owned(),
                },
                value: KvValue::Json(vec!["bar".to_owned()].into()),
                strategy: Strategy::Merge,
            }),
        ];

        for (s, expected) in cases {
            let actual = KvAssignment::from_str(s).unwrap();
            assert_eq!(actual, expected);
        }
    }

    #[test]
    fn test_kv_key_trim_prefix() {
        let mut key = KvKey {
            delim: KeyDelim::Dot,
            path: String::new(),
            full_path: String::new(),
        };

        key.delim = KeyDelim::Dot;
        key.path = "foobar.baz".to_owned();
        assert!(key.trim_prefix("foobar"));
        assert_eq!(key.path, "baz");

        key.path = "foobar.baz".to_owned();
        assert!(!key.trim_prefix("foo"));
        assert_eq!(key.path, "foobar.baz");

        key.path = "foobar".to_owned();
        assert!(key.trim_prefix("foobar"));
        assert_eq!(key.path, "");
        //
        key.path = "foobar".to_owned();
        assert!(!key.trim_prefix("foo"));
        assert_eq!(key.path, "foobar");
    }

    #[test]
    fn test_kv_assignment_try_from_cli_env() {
        let cases = vec![
            ("foo", "", "bar", KvAssignment {
                key: KvKey {
                    path: "foo".to_owned(),
                    delim: KeyDelim::Dot,
                    full_path: "foo".to_owned(),
                },
                value: KvValue::String("bar".to_owned()),
                strategy: Strategy::Set,
            }),
            ("foo", "+", "bar", KvAssignment {
                key: KvKey {
                    path: "foo".to_owned(),
                    delim: KeyDelim::Dot,
                    full_path: "foo".to_owned(),
                },
                value: KvValue::String("bar".to_owned()),
                strategy: Strategy::Merge,
            }),
            ("foo.bar", "", "baz", KvAssignment {
                key: KvKey {
                    path: "foo.bar".to_owned(),
                    delim: KeyDelim::Dot,
                    full_path: "foo.bar".to_owned(),
                },
                value: KvValue::String("baz".to_owned()),
                strategy: Strategy::Set,
            }),
            ("foo.bar", "+", "baz", KvAssignment {
                key: KvKey {
                    path: "foo.bar".to_owned(),
                    delim: KeyDelim::Dot,
                    full_path: "foo.bar".to_owned(),
                },
                value: KvValue::String("baz".to_owned()),
                strategy: Strategy::Merge,
            }),
            ("foo.bar.1", "", "qux", KvAssignment {
                key: KvKey {
                    path: "foo.bar.1".to_owned(),
                    delim: KeyDelim::Dot,
                    full_path: "foo.bar.1".to_owned(),
                },
                value: KvValue::String("qux".to_owned()),
                strategy: Strategy::Set,
            }),
            ("foo.bar.1", "+", "qux", KvAssignment {
                key: KvKey {
                    path: "foo.bar.1".to_owned(),
                    delim: KeyDelim::Dot,
                    full_path: "foo.bar.1".to_owned(),
                },
                value: KvValue::String("qux".to_owned()),
                strategy: Strategy::Merge,
            }),
            ("foo", ":", r#""quux""#, KvAssignment {
                key: KvKey {
                    path: "foo".to_owned(),
                    delim: KeyDelim::Dot,
                    full_path: "foo".to_owned(),
                },
                value: KvValue::Json("quux".into()),
                strategy: Strategy::Set,
            }),
            ("foo", "+:", r#"["quux"]"#, KvAssignment {
                key: KvKey {
                    path: "foo".to_owned(),
                    delim: KeyDelim::Dot,
                    full_path: "foo".to_owned(),
                },
                value: KvValue::Json(vec!["quux".to_owned()].into()),
                strategy: Strategy::Merge,
            }),
        ];

        for (k, mods, v, mut expected) in cases {
            for delim in [KeyDelim::Dot, KeyDelim::Underscore] {
                expected.key.delim = delim;
                let actual = match delim {
                    KeyDelim::Dot => KvAssignment::try_from_cli(format!("{k}{mods}"), v),
                    KeyDelim::Underscore => KvAssignment::try_from_env(k, &format!("{mods}{v}")),
                };

                assert_eq!(actual.unwrap(), expected);
            }
        }
    }

    #[test]
    fn test_kv_assignment_try_from_cli_env_escaped_chars() {
        let cases = vec![
            ("foo=+bar", KvAssignment {
                key: KvKey {
                    path: "foo".to_owned(),
                    delim: KeyDelim::Underscore,
                    full_path: "foo".to_owned(),
                },
                value: KvValue::String("bar".to_owned()),
                strategy: Strategy::Merge,
            }),
            ("foo=\\+bar", KvAssignment {
                key: KvKey {
                    path: "foo".to_owned(),
                    delim: KeyDelim::Underscore,
                    full_path: "foo".to_owned(),
                },
                value: KvValue::String("+bar".to_owned()),
                strategy: Strategy::Set,
            }),
            ("foo=\\:bar", KvAssignment {
                key: KvKey {
                    path: "foo".to_owned(),
                    delim: KeyDelim::Underscore,
                    full_path: "foo".to_owned(),
                },
                value: KvValue::String(":bar".to_owned()),
                strategy: Strategy::Set,
            }),
            ("foo=:true", KvAssignment {
                key: KvKey {
                    path: "foo".to_owned(),
                    delim: KeyDelim::Underscore,
                    full_path: "foo".to_owned(),
                },
                value: KvValue::Json(true.into()),
                strategy: Strategy::Set,
            }),
        ];

        for (s, expected) in cases {
            let (k, v) = s.split_once('=').unwrap();
            let actual = KvAssignment::try_from_env(k, v).unwrap();
            assert_eq!(actual, expected);
        }
    }

    #[test]
    fn test_kv_assignment_try_object() {
        #[derive(Debug, PartialEq, serde::Deserialize)]
        struct Test {
            foo: String,
        }

        let kv = KvAssignment::try_from_cli(":", r#"{"foo":"bar"}"#).unwrap();
        assert_eq!(kv.try_object::<Test>().unwrap(), Test { foo: "bar".into() });

        let kv = KvAssignment::try_from_cli("foo", r#""bar""#).unwrap();
        assert_matches!(
            kv.try_object::<Test>().unwrap_err().error,
            KvAssignmentErrorKind::Type { need, .. } if need == ["object"]
        );
    }

    #[test]
    fn test_kv_assignment_try_object_or_from_str() {
        #[derive(Debug, PartialEq, serde::Deserialize)]
        struct Test {
            foo: String,
        }

        impl FromStr for Test {
            type Err = BoxedError;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                Ok(Self { foo: s.to_owned() })
            }
        }

        let kv = KvAssignment::try_from_cli(":", r#"{"foo":"bar"}"#).unwrap();
        assert_eq!(kv.try_object_or_from_str::<Test, _>().unwrap(), Test {
            foo: "bar".into(),
        });

        let kv = KvAssignment::try_from_cli("foo:", r#""bar""#).unwrap();
        assert_eq!(kv.try_object_or_from_str::<Test, _>().unwrap(), Test {
            foo: "bar".into(),
        });

        let kv = KvAssignment::try_from_cli("foo", "bar").unwrap();
        assert_eq!(kv.try_object_or_from_str::<Test, _>().unwrap(), Test {
            foo: "bar".into(),
        });

        let kv = KvAssignment::try_from_cli("foo:", "42").unwrap();
        assert_matches!(
            kv.try_object_or_from_str::<Test, _>().unwrap_err().error,
            KvAssignmentErrorKind::Type { need, .. } if need == ["object", "string"]
        );
    }

    #[test]
    fn test_kv_assignment_try_string() {
        let kv = KvAssignment::try_from_cli("", "bar").unwrap();
        assert_eq!(kv.try_string().unwrap(), "bar");

        let kv = KvAssignment::try_from_cli(":", r#""bar""#).unwrap();
        assert_eq!(kv.try_string().unwrap(), "bar");

        let kv = KvAssignment::try_from_cli(":", "null").unwrap();
        assert_matches!(
            kv.try_string().unwrap_err().error,
            KvAssignmentErrorKind::Type { need, .. } if need == ["string"]
        );
    }

    #[test]
    fn test_kv_assignment_try_bool() {
        let kv = KvAssignment::try_from_cli("", "true").unwrap();
        assert!(kv.try_bool().unwrap());

        let kv = KvAssignment::try_from_cli(":", "true").unwrap();
        assert!(kv.try_bool().unwrap());

        let kv = KvAssignment::try_from_cli("", "false").unwrap();
        assert!(!kv.try_bool().unwrap());

        let kv = KvAssignment::try_from_cli(":", "false").unwrap();
        assert!(!kv.try_bool().unwrap());

        let kv = KvAssignment::try_from_cli("", "bar").unwrap();
        assert_matches!(
            kv.try_bool().unwrap_err().error,
            KvAssignmentErrorKind::ParseBool { .. }
        );

        let kv = KvAssignment::try_from_cli(":", r#"{"foo":"bar"}"#).unwrap();
        assert_matches!(
            kv.try_bool().unwrap_err().error,
            KvAssignmentErrorKind::Type { need, .. } if need == ["bool", "string"]
        );
    }

    #[test]
    fn test_kv_assignment_try_u32() {
        let kv = KvAssignment::try_from_cli("foo", "42").unwrap();
        assert_eq!(kv.try_u32().unwrap(), 42);

        let kv = KvAssignment::try_from_cli("foo:", "42").unwrap();

        assert_eq!(kv.try_u32().unwrap(), 42);

        let kv = KvAssignment::try_from_cli(":", "true").unwrap();
        assert_matches!(
            kv.try_u32().unwrap_err().error,
            KvAssignmentErrorKind::Type { need, .. } if need == ["number", "string"]
        );

        let kv = KvAssignment::try_from_cli("", "bar").unwrap();
        assert_matches!(
            kv.try_u32().unwrap_err().error,
            KvAssignmentErrorKind::ParseInt { .. }
        );
    }

    #[test]
    #[expect(clippy::too_many_lines)]
    fn test_kv_assignment_try_vec_of_nested() {
        #[derive(Debug, schematic::Config)]
        #[expect(dead_code)]
        struct Test {
            one: String,
            two: String,
        }

        impl FromStr for PartialTest {
            type Err = BoxedError;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                if s == "whoops" {
                    return Err(Box::<dyn std::error::Error + Send + Sync>::from("whoops"));
                }

                Ok(Self {
                    one: Some(s.to_owned()),
                    two: None,
                })
            }
        }

        impl AssignKeyValue for PartialTest {
            fn assign(&mut self, kv: KvAssignment) -> Result<(), BoxedError> {
                match kv.key_string().as_str() {
                    "one" => self.one = Some(kv.try_string()?),
                    "two" => self.two = Some(kv.try_string()?),
                    _ => return missing_key(&kv),
                }

                Ok(())
            }
        }

        let mut v = vec![PartialTest::default()];
        let kv = KvAssignment::try_from_cli("0.one", "bar").unwrap();
        kv.try_vec_of_nested(&mut v).unwrap();
        assert_eq!(v[0].one, Some("bar".into()));

        let mut v = vec![PartialTest::default()];
        let kv = KvAssignment::try_from_cli(":", r#"[{ "one": "1" }, { "two": "2" }]"#).unwrap();
        kv.try_vec_of_nested(&mut v).unwrap();
        assert_eq!(v, vec![
            PartialTest {
                one: Some("1".into()),
                two: None
            },
            PartialTest {
                one: None,
                two: Some("2".into()),
            }
        ]);

        let mut v = vec![PartialTest::default()];
        let kv = KvAssignment::try_from_cli("0:", r#"{ "one": "qux" }"#).unwrap();
        kv.try_vec_of_nested(&mut v).unwrap();
        assert_eq!(v[0].one, Some("qux".into()));

        let mut v = vec![PartialTest::default()];
        let kv = KvAssignment::try_from_cli("0", "quux").unwrap();
        kv.try_vec_of_nested(&mut v).unwrap();
        assert_eq!(v[0].one, Some("quux".into()));

        let mut v = vec![PartialTest::default()];
        let kv = KvAssignment::try_from_cli(":", "[]").unwrap();
        kv.try_vec_of_nested(&mut v).unwrap();
        assert!(v.is_empty());

        let mut v = vec![PartialTest {
            one: None,
            two: Some("foo".into()),
        }];
        let kv = KvAssignment::try_from_cli("+", "bar").unwrap();
        kv.try_vec_of_nested(&mut v).unwrap();
        assert_eq!(v, vec![
            PartialTest {
                one: None,
                two: Some("foo".into()),
            },
            PartialTest {
                one: Some("bar".into()),
                two: None
            }
        ]);

        let mut v = vec![PartialTest::default()];
        let kv = KvAssignment::try_from_cli("one", "qux").unwrap();
        let error = kv.try_vec_of_nested(&mut v).unwrap_err();
        assert_eq!(error.to_string(), "one: type error");

        let mut v = vec![PartialTest::default()];
        let kv = KvAssignment::try_from_cli("1.one", "qux").unwrap();
        let error = kv.try_vec_of_nested(&mut v).unwrap_err();
        assert_eq!(error.to_string(), "1.one: unknown index");

        let mut v = vec![PartialTest::default()];
        let kv = KvAssignment::try_from_cli("0.three", "qux").unwrap();
        let error = kv.try_vec_of_nested(&mut v).unwrap_err();
        assert_eq!(error.to_string(), "0.three: unknown key");

        let mut v = vec![PartialTest::default()];
        let kv = KvAssignment::try_from_cli("0:", "true").unwrap();
        let error = kv.try_vec_of_nested(&mut v).unwrap_err();
        assert_eq!(error.to_string(), "0: type error");

        let mut v = vec![PartialTest::default()];
        let kv = KvAssignment::try_from_cli("0.one:", "true").unwrap();
        let error = kv.try_vec_of_nested(&mut v).unwrap_err();
        assert_eq!(error.to_string(), "0.one: type error");

        let mut v = vec![PartialTest::default()];
        let kv = KvAssignment::try_from_cli("0", "whoops").unwrap();
        let error = kv.try_vec_of_nested(&mut v).unwrap_err();
        assert_eq!(error.to_string(), "0: parse error");

        let mut v = vec![PartialTest::default()];
        let kv = KvAssignment::try_from_cli(":", "42").unwrap();
        let error = kv.try_vec_of_nested(&mut v).unwrap_err();
        assert_eq!(error.to_string(), ": type error");
    }

    #[test]
    fn test_kv_assignment_try_vec_of_strings() {
        let mut v = vec!["foo".to_owned()];
        let kv = KvAssignment::try_from_cli("", "bar").unwrap();
        kv.try_vec_of_strings(&mut v).unwrap();
        assert_eq!(v, vec!["bar".to_owned()]);

        let mut v: Vec<String> = vec![];
        let kv = KvAssignment::try_from_cli("", "foo,bar").unwrap();
        kv.try_vec_of_strings(&mut v).unwrap();
        assert_eq!(v, vec!["foo".to_owned(), "bar".to_owned()]);

        let mut v = vec!["foo".to_owned()];
        let kv = KvAssignment::try_from_cli("0", "bar").unwrap();
        kv.try_vec_of_strings(&mut v).unwrap();
        assert_eq!(v, vec!["bar".to_owned()]);

        let mut v = vec!["foo".to_owned()];
        let kv = KvAssignment::try_from_cli("2", "bar").unwrap();
        let error = kv.try_vec_of_strings(&mut v).unwrap_err();
        assert_eq!(&error.to_string(), "2: unknown index");
    }
}
