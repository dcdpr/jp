use std::{
    env,
    error::Error,
    ffi::OsString,
    hash::Hasher,
    ops::{Deref, DerefMut},
    pin::Pin,
};

use camino::Utf8Path;
use dyn_clone::DynClone;
use dyn_hash::DynHash;
pub use linkme::{self, distributed_slice};
use serde::{Deserialize, Serialize};
pub use typetag;
use url::Url;

#[distributed_slice]
pub static HANDLERS: [fn() -> BoxedHandler] = [..];

/// Names of the environment variables passed to every subprocess an attachment
/// handler spawns.
///
/// `PATH` lets the subprocess find executables, and `HOME` lets it find
/// user-level configuration.
/// On Windows, `USERPROFILE` provides the user profile directory and
/// `SystemRoot` is required to load system libraries.
pub const BASE_ENV_VARS: &[&str] = if cfg!(windows) {
    &["PATH", "HOME", "USERPROFILE", "SystemRoot"]
} else {
    &["PATH", "HOME"]
};

/// Query parameter of an attachment URL that names an environment variable to
/// pass to the spawned subprocess.
///
/// Repeat the parameter to pass multiple variables:
/// `cmd://printenv?arg=TERM&env=TERM`.
/// Handlers write this canonical spelling when rendering a URL; [`env_names`]
/// also accepts `envs`, `env[]`, and `envs[]`.
pub const ENV_PARAM: &str = "env";

/// Accepted query parameter spellings for [`ENV_PARAM`].
///
/// Config tables list variable names under `params.envs = ["TERM", "SHELL"]`,
/// matching `params.args`; `env[]` and `envs[]` match the bracketed spellings
/// accepted for `arg[]` and `args[]`.
const ENV_PARAM_SPELLINGS: &[&str] = &[ENV_PARAM, "envs", "env[]", "envs[]"];

/// Finds the first registered attachment handler to handle the given scheme.
#[must_use]
pub fn find_handler_by_scheme(scheme: &str) -> Option<BoxedHandler> {
    HANDLERS
        .iter()
        .map(|handler| handler())
        .find(|handler| handler.scheme() == scheme)
}

/// A piece of data that can be attached to a conversation.
#[derive(Debug, Clone, PartialEq)]
pub struct Attachment {
    /// The source of the attachment, such as a URL or file path.
    pub source: String,

    /// An optional description of the attachment.
    pub description: Option<String>,

    /// The content of the attachment.
    pub content: AttachmentContent,
}

impl Attachment {
    /// Create a text attachment.
    pub fn text(source: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            source: source.into(),
            description: None,
            content: AttachmentContent::Text(content.into()),
        }
    }

    /// Create a binary attachment with the given MIME type.
    pub fn binary(source: impl Into<String>, data: Vec<u8>, media_type: impl Into<String>) -> Self {
        Self {
            source: source.into(),
            description: None,
            content: AttachmentContent::Binary {
                data,
                media_type: media_type.into(),
            },
        }
    }

    /// Set the description of the attachment.
    #[must_use]
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Returns `true` if this attachment contains text content.
    #[must_use]
    pub const fn is_text(&self) -> bool {
        matches!(self.content, AttachmentContent::Text(_))
    }

    /// Returns `true` if this attachment contains binary content.
    #[must_use]
    pub const fn is_binary(&self) -> bool {
        matches!(self.content, AttachmentContent::Binary { .. })
    }

    /// Returns the text content, if this is a text attachment.
    #[must_use]
    pub fn as_text(&self) -> Option<&str> {
        match &self.content {
            AttachmentContent::Text(s) => Some(s),
            AttachmentContent::Binary { .. } => None,
        }
    }
}

/// The content of an attachment, either UTF-8 text or binary data.
#[derive(Debug, Clone, PartialEq)]
pub enum AttachmentContent {
    /// UTF-8 text content (source code, markdown, config files, etc.)
    Text(String),

    /// Binary content with a MIME type (images, PDFs, etc.)
    Binary {
        /// The raw bytes.
        data: Vec<u8>,
        /// MIME type, e.g. `"image/png"`, `"application/pdf"`.
        media_type: String,
    },
}

/// A trait for handling attachments.
///
/// Any type that implements this trait can be used to handle a set of
/// attachment types.
///
/// For example, a `file` handler could handle attachments that are files on the
/// local file system, while a `web` handler could handle attachments that are
/// URLs pointing to web pages.
///
/// Handlers are stored and serialized as `Box<dyn Handler>`, so each async
/// method returns a [`HandlerFuture`]; an implementation writes its body as
/// `Box::pin(async move { … })`.
#[typetag::serde(tag = "type")]
pub trait Handler: std::fmt::Debug + DynClone + DynHash + Send + Sync {
    /// The URI scheme of the handler.
    ///
    /// This is used to determine which handler to use for a given URI.
    /// The scheme has to be unique across all handlers.
    fn scheme(&self) -> &'static str;

    /// Return the canonical form of `uri`.
    ///
    /// Two URLs with the same canonical form resolve to identical attachment
    /// content, so callers compare canonical URLs to deduplicate attachments.
    /// Equivalent spellings (shorthands vs. full URLs, parameter aliases, or
    /// order-independent query values) normalize to a single URL.
    /// Calling `canonical` on an already-canonical URL returns that URL
    /// unchanged.
    ///
    /// Performs no I/O, so callers can validate and canonicalize a URL before
    /// resolving or storing it.
    ///
    /// # Errors
    ///
    /// Returns an error if `uri` is invalid for this handler.
    /// [`Self::add`] may still reject a URL accepted here for reasons that
    /// require I/O (such as a missing or oversized file).
    fn canonical(&self, uri: &Url) -> Result<Url, Box<dyn Error + Send + Sync>>;

    /// Build the URL for an attachment written as a config table, from its
    /// `path`.
    ///
    /// The caller adds the table's `params` as query parameters.
    /// The default is `<scheme>://<path>`, which puts the start of `path` in
    /// the URL's authority.
    /// A handler whose `path` belongs elsewhere in its URL overrides this.
    ///
    /// # Errors
    ///
    /// Returns an error if `path` does not form a valid URL.
    fn table_url(&self, path: &str) -> Result<Url, Box<dyn Error + Send + Sync>> {
        Ok(Url::parse(&format!("{}://{path}", self.scheme()))?)
    }

    /// Add a new attachment, using the given URL.
    ///
    /// The `cwd` parameter is the current working directory, used by handlers
    /// that need to resolve relative paths for validation (e.g. checking file
    /// existence or size).
    fn add<'a>(&'a mut self, uri: &'a Url, cwd: &'a Utf8Path) -> HandlerFuture<'a, ()>;

    /// Remove an attachment, using the given URL.
    fn remove<'a>(&'a mut self, uri: &'a Url) -> HandlerFuture<'a, ()>;

    /// List all attachment URIs handled by this handler.
    fn list(&self) -> HandlerFuture<'_, Vec<Url>>;

    /// Return all the attachments handled by this handler.
    ///
    /// The `cwd` parameter is the current working directory, and can be used to
    /// resolve relative paths.
    fn get<'a>(&'a self, cwd: &'a Utf8Path) -> HandlerFuture<'a, Vec<Attachment>>;
}

/// The future a [`Handler`] method returns, resolving to `T` or an error.
pub type HandlerFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, Box<dyn Error + Send + Sync>>> + Send + 'a>>;

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

/// Extract the environment variable names specified by [`ENV_PARAM`] query
/// parameters in `uri`, sorted and deduplicated.
///
/// Accepts the parameter spellings `env`, `envs`, `env[]`, and `envs[]`.
///
/// # Errors
///
/// Returns an error if a variable name is empty or contains `=` or a NUL byte.
/// The parameter forwards an existing variable from the parent environment and
/// cannot assign a new value.
pub fn env_names(uri: &Url) -> Result<Vec<String>, Box<dyn Error + Send + Sync>> {
    let mut names = vec![];
    for (key, name) in uri.query_pairs() {
        if !ENV_PARAM_SPELLINGS.contains(&key.as_ref()) {
            continue;
        }

        if name.is_empty() || name.contains(['=', '\0']) {
            return Err(format!(
                "invalid `{key}` value `{name}`: expected the name of an environment variable, \
                 such as `TERM`"
            )
            .into());
        }

        names.push(name.into_owned());
    }

    names.sort();
    names.dedup();

    Ok(names)
}

/// Build the isolated environment for a subprocess spawned by an attachment
/// handler.
///
/// Includes each variable in [`BASE_ENV_VARS`] and `names` that is set in the
/// current process.
/// Variables not in either list are omitted.
#[must_use]
pub fn child_env(names: &[String]) -> Vec<(OsString, OsString)> {
    child_env_with(names, |name| env::var_os(name))
}

fn child_env_with(
    names: &[String],
    lookup: impl Fn(&str) -> Option<OsString>,
) -> Vec<(OsString, OsString)> {
    BASE_ENV_VARS
        .iter()
        .copied()
        .chain(names.iter().map(String::as_str))
        .filter_map(|name| Some((OsString::from(name), lookup(name)?)))
        .collect()
}

#[cfg(test)]
#[path = "lib_tests.rs"]
mod tests;
