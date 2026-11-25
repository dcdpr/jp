//! The `mcp` attachment scheme, kept readable but no longer resolvable.
//!
//! Conversations recorded before MCP resource attachments were retired still
//! carry `mcp+<server>+<scheme>://` entries under the `mcp` handler tag.
//! This handler keeps deserializing, listing, and removing them so those
//! conversations load, are inspectable, and can be edited.
//! Resolving one reports [`UnsupportedResolution`] instead of reading from an
//! MCP server.

use std::{collections::BTreeSet, error::Error, fmt};

use camino::Utf8Path;
use jp_attachment::{
    Attachment, BoxedHandler, HANDLERS, Handler, HandlerFuture, distributed_slice, linkme, typetag,
};
use serde::{Deserialize, Serialize};
use url::Url;

#[distributed_slice(HANDLERS)]
#[linkme(crate = linkme)]
static HANDLER: fn() -> BoxedHandler = handler;

fn handler() -> BoxedHandler {
    (Box::new(McpResources::default()) as Box<dyn Handler>).into()
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct McpResources(BTreeSet<Url>);

/// Returned when an `mcp` attachment is asked for its contents.
///
/// Names every attachment the conversation carries, so one query tells the user
/// the whole set to remove rather than one per attempt.
#[derive(Debug)]
pub struct UnsupportedResolution {
    uris: Vec<Url>,
}

impl fmt::Display for UnsupportedResolution {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let list = self
            .uris
            .iter()
            .map(|uri| format!("`{uri}`"))
            .collect::<Vec<_>>()
            .join(", ");
        let removals = self
            .uris
            .iter()
            .map(|uri| format!("jp attachment rm '{uri}'"))
            .collect::<Vec<_>>()
            .join(" && ");
        write!(
            f,
            "MCP resource attachments are no longer resolved: {list}. Remove them with: {removals}"
        )
    }
}

impl Error for UnsupportedResolution {}

#[typetag::serde(name = "mcp")]
impl Handler for McpResources {
    fn scheme(&self) -> &'static str {
        "mcp"
    }

    // MCP resource attachments are no longer resolved; keep the stored URL
    // verbatim.
    fn canonical(&self, uri: &Url) -> Result<Url, Box<dyn Error + Send + Sync>> {
        Ok(uri.clone())
    }

    fn add<'a>(&'a mut self, uri: &'a Url, _cwd: &'a Utf8Path) -> HandlerFuture<'a, ()> {
        Box::pin(async move {
            self.0.insert(uri.clone());

            Ok(())
        })
    }

    fn remove<'a>(&'a mut self, uri: &'a Url) -> HandlerFuture<'a, ()> {
        Box::pin(async move {
            self.0.remove(uri);

            Ok(())
        })
    }

    fn list(&self) -> HandlerFuture<'_, Vec<Url>> {
        Box::pin(async move { Ok(self.0.clone().into_iter().collect()) })
    }

    fn get<'a>(&'a self, _: &'a Utf8Path) -> HandlerFuture<'a, Vec<Attachment>> {
        Box::pin(async move {
            if self.0.is_empty() {
                return Ok(vec![]);
            }
            Err(UnsupportedResolution {
                uris: self.0.iter().cloned().collect(),
            }
            .into())
        })
    }
}

#[cfg(test)]
#[path = "lib_tests.rs"]
mod tests;
