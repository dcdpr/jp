//! The `mcp` attachment scheme, kept readable but no longer resolvable.
//!
//! Conversations recorded before MCP resource attachments were retired still
//! carry `mcp+<server>+<scheme>://` entries under the `mcp` handler tag.
//! This handler keeps deserializing, listing, and removing them so those
//! conversations load, are inspectable, and can be edited.
//! Resolving one reports [`UnsupportedResolution`] instead of reading from an
//! MCP server, and adding a new one is refused with [`Retired`].

use std::{collections::BTreeSet, error::Error, fmt};

use async_trait::async_trait;
use camino::Utf8Path;
use jp_attachment::{
    Attachment, BoxedHandler, HANDLERS, Handler, distributed_slice, linkme, typetag,
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
/// Names every URI the handler holds, each with the `jp attachment rm` command
/// that removes it.
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

/// Returned when an `mcp` attachment is canonicalized, which refuses it before
/// it is added to a conversation.
#[derive(Debug)]
pub struct Retired {
    uri: Url,
}

impl fmt::Display for Retired {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "MCP resource attachments are retired and can no longer be added: `{}`",
            self.uri
        )
    }
}

impl Error for Retired {}

#[typetag::serde(name = "mcp")]
#[async_trait]
impl Handler for McpResources {
    fn scheme(&self) -> &'static str {
        "mcp"
    }

    // Refusing here keeps a new URI from being stored. One already stored is
    // still listed, removed, and reported by `get`.
    fn canonical(&self, uri: &Url) -> Result<Url, Box<dyn Error + Send + Sync>> {
        Err(Box::new(Retired { uri: uri.clone() }))
    }

    async fn add(
        &mut self,
        uri: &Url,
        _cwd: &Utf8Path,
    ) -> Result<(), Box<dyn Error + Send + Sync>> {
        self.0.insert(uri.clone());

        Ok(())
    }

    async fn remove(&mut self, uri: &Url) -> Result<(), Box<dyn Error + Send + Sync>> {
        self.0.remove(uri);

        Ok(())
    }

    async fn list(&self) -> Result<Vec<Url>, Box<dyn Error + Send + Sync>> {
        Ok(self.0.clone().into_iter().collect())
    }

    async fn get(&self, _: &Utf8Path) -> Result<Vec<Attachment>, Box<dyn Error + Send + Sync>> {
        if self.0.is_empty() {
            return Ok(vec![]);
        }
        Err(Box::new(UnsupportedResolution {
            uris: self.0.iter().cloned().collect(),
        }))
    }
}

#[cfg(test)]
#[path = "lib_tests.rs"]
mod tests;
