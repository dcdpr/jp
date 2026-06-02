//! Parsing and formatting of `ag://` attachment URIs.

use std::error::Error;

use serde::{Deserialize, Serialize};
use url::Url;

const SCHEME: &str = "ag";

/// A resource namespace within the tracker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub(crate) enum Namespace {
    Issues,
}

impl Namespace {
    /// The canonical, lowercase path segment for this namespace.
    fn as_str(self) -> &'static str {
        match self {
            Namespace::Issues => "issues",
        }
    }

    /// Parse a path segment into a namespace, accepting singular and plural.
    fn parse(segment: &str) -> Option<Self> {
        match segment {
            "issue" | "issues" => Some(Namespace::Issues),
            _ => None,
        }
    }
}

/// A reference to a single tracker resource, e.g. issue 592.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub(crate) struct Reference {
    namespace: Namespace,
    id: String,
}

impl Reference {
    pub(crate) fn namespace(&self) -> Namespace {
        self.namespace
    }

    pub(crate) fn id(&self) -> &str {
        &self.id
    }

    /// Parse any supported `ag://` URI spelling into a reference.
    ///
    /// Accepts both the hierarchical (`ag://issues/592`) and opaque
    /// (`ag:issues/592`) forms, the singular and plural namespace spelling, and
    /// the bare-number shorthand (`ag:592`) which defaults to `issues`.
    pub(crate) fn parse(uri: &Url) -> Result<Self, Box<dyn Error + Send + Sync>> {
        if uri.scheme() != SCHEME {
            return Err(format!("expected `{SCHEME}` scheme, got `{}`", uri.scheme()).into());
        }

        let segments = if uri.cannot_be_a_base() {
            // Opaque form, e.g. `ag:issues/592` or `ag:592`.
            uri.path()
                .split('/')
                .filter(|s| !s.is_empty())
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>()
        } else if let Some(host) = uri.host_str() {
            // Hierarchical form, e.g. `ag://issues/592` or `ag://592`.
            let mut segments = vec![host.to_owned()];
            segments.extend(
                uri.path_segments()
                    .into_iter()
                    .flatten()
                    .filter(|s| !s.is_empty())
                    .map(ToOwned::to_owned),
            );
            segments
        } else {
            return Err(unsupported(uri));
        };

        let (namespace, id) = match segments.as_slice() {
            [id] => (Namespace::Issues, id.as_str()),
            [namespace, id] => (
                Namespace::parse(namespace).ok_or_else(|| unsupported(uri))?,
                id.as_str(),
            ),
            _ => return Err(unsupported(uri)),
        };

        validate_id(id)?;

        Ok(Self {
            namespace,
            id: id.to_owned(),
        })
    }

    /// Render the reference back into its canonical hierarchical URI.
    pub(crate) fn to_url(&self) -> Result<Url, Box<dyn Error + Send + Sync>> {
        Url::parse(&format!(
            "{SCHEME}://{}/{}",
            self.namespace.as_str(),
            self.id
        ))
        .map_err(Into::into)
    }
}

fn validate_id(id: &str) -> Result<(), Box<dyn Error + Send + Sync>> {
    if id.is_empty() || !id.bytes().all(|b| b.is_ascii_digit()) {
        return Err(format!("invalid issue id `{id}`: expected a number").into());
    }
    Ok(())
}

fn unsupported(uri: &Url) -> Box<dyn Error + Send + Sync> {
    format!("unsupported ag URI `{uri}`; expected `ag://issues/N`, `ag:issues/N`, or `ag:N`").into()
}

#[cfg(test)]
#[path = "uri_tests.rs"]
mod tests;
