use std::{
    hash::Hasher,
    ops::{Deref, DerefMut},
    path::Path,
};

use dyn_clone::DynClone;
use dyn_hash::DynHash;
pub use linkme::{self, distributed_slice};
use serde::{Deserialize, Serialize};
pub use typetag;
use url::Url;

#[distributed_slice]
pub static HANDLERS: [fn() -> BoxedHandler] = [..];

/// Finds the first registered attachment handler to handle the given scheme.
#[must_use]
pub fn find_handler_by_scheme(scheme: &str) -> Option<BoxedHandler> {
    HANDLERS
        .iter()
        .map(|handler| handler())
        .find(|handler| handler.scheme() == scheme)
}

/// A piece of data that can be attached to a conversation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Attachment {
    /// The source of the attachment, such as a URL or file path.
    pub source: String,

    /// The content of the attachment.
    ///
    /// This can be the content as-is, or a JSON or XML representation of any
    /// structured data that is relevant to the attachment.
    pub content: String,
}

/// A trait for handling attachments.
///
/// Any type that implements this trait can be used to handle a set of
/// attachment types.
///
/// For example, a `file` handler could handle attachments that are files
/// on the local file system, while a `web` handler could handle attachments
/// that are URLs pointing to web pages.
#[typetag::serde(tag = "type")]
pub trait Handler: std::fmt::Debug + DynClone + DynHash {
    /// The URI scheme of the handler.
    ///
    /// This is used to determine which handler to use for a given URI. The
    /// scheme has to be unique across all handlers.
    fn scheme(&self) -> &'static str;

    /// Add a new attachment, using the given URL.
    fn add(&mut self, uri: &Url) -> Result<(), Box<dyn std::error::Error>>;

    /// Remove an attachment, using the given URL.
    fn remove(&mut self, uri: &Url) -> Result<(), Box<dyn std::error::Error>>;

    /// List all attachment URIs handled by this handler.
    fn list(&self) -> Result<Vec<Url>, Box<dyn std::error::Error>>;

    /// Return all the attachments handled by this handler.
    ///
    /// The `cwd` parameter is the current working directory, and can be used to
    /// resolve relative paths.
    fn get(&self, cwd: &Path) -> Result<Vec<Attachment>, Box<dyn std::error::Error>>;
}

dyn_clone::clone_trait_object!(Handler);
dyn_hash::hash_trait_object!(Handler);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BoxedHandler(Box<dyn Handler>);

impl Deref for BoxedHandler {
    type Target = dyn Handler;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}

impl DerefMut for BoxedHandler {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.0.as_mut()
    }
}

impl PartialEq for BoxedHandler {
    fn eq(&self, other: &Self) -> bool {
        self.0.scheme() == other.0.scheme()
    }
}

impl Eq for BoxedHandler {}

impl std::hash::Hash for BoxedHandler {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.scheme().hash(state);
    }
}

impl PartialOrd for BoxedHandler {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for BoxedHandler {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.0.scheme().cmp(other.0.scheme())
    }
}

impl From<Box<dyn Handler>> for BoxedHandler {
    fn from(value: Box<dyn Handler>) -> Self {
        Self(value)
    }
}
