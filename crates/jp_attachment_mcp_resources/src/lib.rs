use std::{collections::BTreeSet, error::Error, path::Path};

use async_trait::async_trait;
use jp_attachment::{
    Attachment, BoxedHandler, HANDLERS, Handler, distributed_slice, linkme, typetag,
};
use jp_mcp::{Client, ResourceContents, id::McpServerId};
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

/// Output from a command.
#[derive(Debug, Clone, PartialEq, Serialize)]
struct Resource(Vec<String>);

impl Resource {
    pub fn try_to_xml(&self) -> Result<String, Box<dyn Error + Send + Sync>> {
        let mut buffer = String::new();
        let mut serializer = quick_xml::se::Serializer::new(&mut buffer);
        serializer.indent(' ', 2);
        self.serialize(serializer)?;
        Ok(buffer)
    }
}

impl From<Vec<ResourceContents>> for Resource {
    fn from(contents: Vec<ResourceContents>) -> Self {
        Resource(
            contents
                .into_iter()
                .filter_map(|c| match c {
                    ResourceContents::TextResourceContents { text, .. } => Some(text),
                    ResourceContents::BlobResourceContents { .. } => None,
                })
                .collect(),
        )
    }
}

#[typetag::serde(name = "mcp")]
#[async_trait]
impl Handler for McpResources {
    fn scheme(&self) -> &'static str {
        "mcp"
    }

    async fn add(&mut self, uri: &Url) -> Result<(), Box<dyn Error + Send + Sync>> {
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

    async fn get(
        &self,
        _: &Path,
        client: Client,
    ) -> Result<Vec<Attachment>, Box<dyn Error + Send + Sync>> {
        let mut attachments = vec![];
        for uri in &self.0 {
            // "mcp+github-mcp-server+repo" -> ("mcp+github-mcp-server", "repo")
            let (mcp, scheme) = uri.scheme().rsplit_once('+').unwrap_or(("", uri.scheme()));

            // "mcp+github-mcp-server" -> "github-mcp-server"
            let server_id = McpServerId::new(mcp.split_once('+').unwrap_or(("", mcp)).1);

            let mut resource_uri = uri.clone();
            let _ = resource_uri.set_scheme(scheme);

            let resource = client
                .get_resource_contents(&server_id, resource_uri)
                .await?;

            attachments.push(Attachment {
                source: uri.to_string(),
                content: Resource::from(resource).try_to_xml()?,
                ..Default::default()
            });
        }

        Ok(attachments)
    }
}
