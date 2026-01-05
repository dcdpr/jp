use std::{
    error::Error,
    hash::Hasher,
    ops::{Deref, DerefMut},
    path::Path,
};

use async_trait::async_trait;
use dyn_clone::DynClone;
use dyn_hash::DynHash;
use jp_mcp::Client;
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
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Attachment {
    /// The source of the attachment, such as a URL or file path.
    pub source: String,

    /// An optional description of the attachment.
    pub description: Option<String>,

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
#[async_trait]
pub trait Handler: std::fmt::Debug + DynClone + DynHash + Send + Sync {
    /// The URI scheme of the handler.
    ///
    /// This is used to determine which handler to use for a given URI. The
    /// scheme has to be unique across all handlers.
    fn scheme(&self) -> &'static str;

    /// Add a new attachment, using the given URL.
    async fn add(&mut self, uri: &Url) -> Result<(), Box<dyn Error + Send + Sync>>;

    /// Remove an attachment, using the given URL.
    async fn remove(&mut self, uri: &Url) -> Result<(), Box<dyn Error + Send + Sync>>;

    /// List all attachment URIs handled by this handler.
    async fn list(&self) -> Result<Vec<Url>, Box<dyn Error + Send + Sync>>;

    /// Return all the attachments handled by this handler.
    ///
    /// The `root` parameter is the root working directory, and can be used to
    /// resolve relative paths.
    ///
    /// The `mcp_client` parameter is the MCP client to use for fetching
    /// resources from MCP servers, if needed.
    async fn get(
        &self,
        root: &Path,
        mcp_client: Client,
    ) -> Result<Vec<Attachment>, Box<dyn Error + Send + Sync>>;
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

/// Decodes a percent-encoded query parameter value, handling potential UTF-8
/// errors.
pub fn percent_decode_str(encoded: &str) -> Result<String, Box<dyn Error + Send + Sync>> {
    percent_encoding::percent_decode_str(encoded)
        .decode_utf8()
        .map(|s| s.to_string())
        .map_err(Into::into)
}

#[must_use]
pub fn percent_encode_str(encoded: &str) -> String {
    percent_encoding::percent_encode(encoded.as_bytes(), percent_encoding::NON_ALPHANUMERIC)
        .to_string()
}
