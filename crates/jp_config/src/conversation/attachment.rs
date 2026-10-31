//! Attachment configuration for conversations.

use std::str::FromStr;

use indexmap::IndexMap;
use schematic::Config;
use serde_json::Value;
use url::{Position, Url};

use crate::{
    BoxedError, Error,
    assignment::{AssignKeyValue, AssignResult, KvAssignment, missing_key},
    delta::{PartialConfigDelta, delta_opt},
    partial::{ToPartial, partial_opt},
    types::json_value::JsonValue,
};

/// Attachment configuration.
#[derive(Debug, Clone, PartialEq, Config)]
#[config(serde(untagged))]
pub enum AttachmentConfig {
    /// A url-based attachment.
    #[setting(is_empty = false)]
    Url(Url),

    /// Attachment defined as an object.
    #[setting(nested, default)]
    Object(AttachmentObjectConfig),
}

impl AssignKeyValue for PartialAttachmentConfig {
    fn assign(&mut self, kv: KvAssignment) -> AssignResult {
        match kv.key_string().as_str() {
            "" => *self = kv.try_object_or_from_str()?,
            _ => match self {
                Self::Object(object) => object.assign(kv)?,
                // A field assigned to a URL attachment edits that part of the
                // URL: the URL becomes the table that rebuilds it, and the field
                // is assigned there.
                Self::Url(url) => {
                    let mut object = object_form(url).ok_or_else(|| {
                        format!(
                            "`{url}` cannot be edited field by field, because it has no `//` \
                             after its scheme; set the whole attachment instead"
                        )
                    })?;
                    object.assign(kv)?;
                    *self = Self::Object(object);
                }
            },
        }

        Ok(())
    }
}

/// The table that rebuilds `url`: its scheme as `type`, its query as `params`,
/// and everything else after `//` as `path`.
///
/// `None` for a URL without `//` after its scheme, such as `gh:pull/544/diff`.
/// A table always rebuilds `<type>://<path>`, which an attachment handler reads
/// as a different URL.
fn object_form(url: &Url) -> Option<PartialAttachmentObjectConfig> {
    if !url.has_authority() {
        return None;
    }

    // The fragment stays on the path, where the rebuilt URL keeps it.
    let path = format!(
        "{}{}",
        &url[Position::BeforeUsername..Position::AfterPath],
        &url[Position::AfterQuery..]
    );

    let mut values = IndexMap::<String, Vec<String>>::new();
    for (key, value) in url.query_pairs() {
        values
            .entry(key.into_owned())
            .or_default()
            .push(value.into_owned());
    }

    // A parameter given more than once becomes a list, which is written back as
    // the same repeated parameter.
    let params: IndexMap<_, _> = values
        .into_iter()
        .map(|(key, mut values)| {
            let value = if values.len() == 1 {
                Value::String(values.remove(0))
            } else {
                Value::from(values)
            };
            (key, JsonValue(value))
        })
        .collect();

    Some(PartialAttachmentObjectConfig {
        kind: Some(url.scheme().to_owned()),
        path: Some(path),
        params: (!params.is_empty()).then_some(params),
    })
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
        Self::from_partial(partial, vec![]).map_err(Into::into)
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

/// An attachment written as a table.
///
/// Its URL is `<type>://<path>`, with each `params` entry added as a query
/// parameter.
#[derive(Debug, Clone, PartialEq, Config)]
pub struct AttachmentObjectConfig {
    /// The type of the attachment.
    ///
    /// e.g. `file`, `http`, etc.
    #[setting(required, rename = "type")]
    pub kind: String,

    /// The url path of the attachment.
    ///
    /// The path part of the URL.
    #[setting(required)]
    pub path: String,

    /// The query parameters of the attachment.
    pub params: IndexMap<String, JsonValue>,
}

impl AssignKeyValue for PartialAttachmentObjectConfig {
    fn assign(&mut self, mut kv: KvAssignment) -> AssignResult {
        match kv.key_string().as_str() {
            "" => kv.try_merge_object(self)?,
            "type" => self.kind = kv.try_some_string()?,
            "path" => self.path = kv.try_some_string()?,
            _ if kv.p("params") => kv.assign_to_entry(self.params.get_or_insert_default())?,
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
            match &value.0 {
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
                    ));
                }
            }
        }

        Ok(url)
    }
}

#[cfg(test)]
#[path = "attachment_tests.rs"]
mod tests;
