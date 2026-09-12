//! The `mcp` attachment scheme, kept readable but no longer resolvable.
//!
//! Conversations recorded before MCP resource attachments were retired still
//! carry `mcp+<server>+<scheme>://` entries under the `mcp` handler tag.
//! This handler keeps deserializing, listing, and removing them so those
//! conversations load, are inspectable, and can be edited.
//! Resolving one reports [`UnsupportedResolution`] instead of reading from an
//! MCP server.

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
/// Names the attachment so a conversation carrying several of them says which
/// one to remove.
#[derive(Debug)]
pub struct UnsupportedResolution {
    uri: Url,
}

impl fmt::Display for UnsupportedResolution {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "MCP resource attachments are no longer resolved: `{}`. Remove it with `jp attachment \
             rm {}`.",
            self.uri, self.uri
        )
    }
}

impl Error for UnsupportedResolution {}

#[typetag::serde(name = "mcp")]
#[async_trait]
impl Handler for McpResources {
    fn scheme(&self) -> &'static str {
        "mcp"
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
        match self.0.iter().next() {
            Some(uri) => Err(Box::new(UnsupportedResolution { uri: uri.clone() })),
            None => Ok(vec![]),
        }
    }
}

#[cfg(test)]
#[path = "lib_tests.rs"]
mod tests;
