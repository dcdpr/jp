//! Title configuration for conversations.

use std::str::FromStr;

use indexmap::IndexMap;
use schematic::Config;
use serde_json::Value;
use url::Url;

use crate::{
    assignment::{missing_key, AssignKeyValue, AssignResult, KvAssignment},
    delta::{delta_opt, PartialConfigDelta},
    partial::{partial_opt, ToPartial},
    BoxedError, Error,
};

/// Reasoning configuration.
#[derive(Debug, Clone, Config)]
#[config(serde(untagged))]
pub enum AttachmentConfig {
    /// A url-based attachment.
    Url(Url),

    /// Attachment defined as an object.
    #[setting(nested, default)]
    Object(AttachmentObjectConfig),
}

impl AssignKeyValue for PartialAttachmentConfig {
    fn assign(&mut self, kv: KvAssignment) -> AssignResult {
        #[expect(clippy::single_match_else)]
        match kv.key_string().as_str() {
            "" => *self = kv.try_object_or_from_str()?,
            _ => {
                let mut object = PartialAttachmentObjectConfig::default();
                object.assign(kv)?;
                *self = Self::Object(object);
            }
        }

        Ok(())
    }
}

impl PartialConfigDelta for PartialAttachmentConfig {
    fn delta(&self, next: Self) -> Self {
        match (self, next) {
            (Self::Object(prev), Self::Object(next)) => Self::Object(prev.delta(next)),
            (_, next) => next,
        }
    }
}

impl ToPartial for AttachmentConfig {
    fn to_partial(&self) -> Self::Partial {
        match self {
            Self::Url(url) => Self::Partial::Url(url.clone()),
            Self::Object(v) => Self::Partial::Object(v.to_partial()),
        }
    }
}

impl FromStr for PartialAttachmentConfig {
    type Err = BoxedError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self::Url(s.parse()?))
    }
}

impl FromStr for AttachmentConfig {
    type Err = BoxedError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let partial = PartialAttachmentConfig::from_str(s)?;
        Self::from_partial(partial).map_err(Into::into)
    }
}

impl AttachmentConfig {
    /// Convert an attachment configuration to a URL.
    ///
    /// # Errors
    ///
    /// See [`AttachmentObjectConfig::to_url`].
    pub fn to_url(&self) -> Result<Url, Error> {
        match self {
            Self::Url(url) => Ok(url.clone()),
            Self::Object(v) => v.to_url(),
        }
    }
}

impl From<Url> for AttachmentConfig {
    fn from(url: Url) -> Self {
        Self::Url(url)
    }
}

impl From<Url> for PartialAttachmentConfig {
    fn from(url: Url) -> Self {
        Self::Url(url)
    }
}

/// Custom reasoning configuration.
#[derive(Debug, Clone, PartialEq, Config)]
pub struct AttachmentObjectConfig {
    /// The type of the attachment.
    #[setting(required, rename = "type")]
    pub kind: String,

    /// The url path of the attachment.
    #[setting(required)]
    pub path: String,

    /// The query parameters of the attachment.
    pub params: IndexMap<String, Value>,
}

impl AssignKeyValue for PartialAttachmentObjectConfig {
    fn assign(&mut self, kv: KvAssignment) -> AssignResult {
        match kv.key_string().as_str() {
            "" => *self = kv.try_object()?,
            "kind" => self.kind = kv.try_some_string()?,
            "path" => self.path = kv.try_some_string()?,
            "params" => kv.try_object()?,
            _ => return missing_key(&kv),
        }

        Ok(())
    }
}

impl PartialConfigDelta for PartialAttachmentObjectConfig {
    fn delta(&self, next: Self) -> Self {
        Self {
            kind: delta_opt(self.kind.as_ref(), next.kind),
            path: delta_opt(self.path.as_ref(), next.path),
            params: delta_opt(self.params.as_ref(), next.params),
        }
    }
}

impl ToPartial for AttachmentObjectConfig {
    fn to_partial(&self) -> Self::Partial {
        let defaults = Self::Partial::default();

        Self::Partial {
            kind: partial_opt(&self.kind, defaults.kind),
            path: partial_opt(&self.path, defaults.path),
            params: partial_opt(&self.params, defaults.params),
        }
    }
}

impl AttachmentObjectConfig {
    /// Convert an attachment configuration to a URL.
    ///
    /// # Errors
    ///
    /// Returns an error if the URL cannot be constructed due to invalid parts.
    pub fn to_url(&self) -> Result<Url, Error> {
        let mut url = format!("{}://{}", self.kind, self.path)
            .parse::<Url>()
            .map_err(|e| Error::Custom(Box::new(e)))?;

        for (key, value) in &self.params {
            match value {
                Value::String(value) => {
                    url.query_pairs_mut().append_pair(key, value);
                }
                Value::Array(values) => {
                    for value in values {
                        let Some(value) = value.as_str() else {
                            return Err(Error::Custom(
                                format!(
                                    "Invalid array item. Expected a string, received {value:?}.",
                                )
                                .into(),
                            ));
                        };

                        url.query_pairs_mut().append_pair(key, value);
                    }
                }
                _ => {
                    return Err(Error::Custom(
                        format!(
                            "Invalid parameter value for key {key}. Expected a string or array of \
                             strings.",
                        )
                        .into(),
                    ))
                }
            }
        }

        Ok(url)
    }
}
