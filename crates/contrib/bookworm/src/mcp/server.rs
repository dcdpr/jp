use std::str::FromStr as _;

use indoc::indoc;
use rmcp::{
    ErrorData as McpError,
    handler::server::{tool::ToolRouter, wrapper::Parameters},
    model::{
        CallToolResult, Content, Implementation, ResourceContents, ServerCapabilities, ServerInfo,
    },
    tool, tool_handler, tool_router,
};
use schemars::JsonSchema;
use serde::Deserialize;
use tracing::debug;
use url::Url;

use crate::{
    error::Error,
    index::EntryType,
    mcp::tools::{CrateUri, PathRoot, format_xml, truncate_resources, valid_crate_version},
    query,
};

/// Configuration for the bookworm MCP server.
#[derive(Debug, Clone, Default)]
pub struct ServerConfig {
    /// Enable JP tool protocol (serialize outputs as `jp_tool::Outcome` JSON).
    pub jp_protocol: bool,
}

#[derive(Clone)]
pub struct BookwormService {
    config: ServerConfig,
    tool_router: ToolRouter<Self>,
}

impl BookwormService {
    fn format_error(&self, err: &Error) -> CallToolResult {
        let msg = err.to_string();
        if self.config.jp_protocol {
            let outcome = jp_tool::Outcome::error(err);
            let json = serde_json::to_string(&outcome).unwrap_or_else(|_| msg.clone());
            CallToolResult::success(vec![Content::text(json)])
        } else {
            CallToolResult::error(vec![Content::text(msg)])
        }
    }

    fn format_contents(&self, contents: Vec<Content>) -> CallToolResult {
        if !self.config.jp_protocol {
            return CallToolResult::success(contents);
        }

        // Concatenate textual parts into a single JP `Success` payload. Embedded
        // resources are flattened to their text bodies so the JP side gets a
        // single string outcome.
        let mut text = String::new();
        for content in contents {
            match content.raw {
                rmcp::model::RawContent::Text(t) => {
                    if !text.is_empty() {
                        text.push('\n');
                    }
                    text.push_str(&t.text);
                }
                rmcp::model::RawContent::Resource(r) => {
                    if !text.is_empty() {
                        text.push('\n');
                    }
                    if let ResourceContents::TextResourceContents { text: body, .. } = r.resource {
                        text.push_str(&body);
                    }
                }
                _ => {}
            }
        }

        let outcome = jp_tool::Outcome::Success { content: text };
        let json = serde_json::to_string(&outcome).unwrap_or_default();
        CallToolResult::success(vec![Content::text(json)])
    }
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SearchCratesRequest {
    /// Search query.
    pub query: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SearchCrateItemsRequest {
    /// The exact name of the crate.
    pub crate_name: String,

    /// The version of the crate.
    /// Either a semantic version or `latest` for the latest published crate
    /// version.
    #[serde(default = "default_latest")]
    pub crate_version: String,

    /// The `query` parameter does partial matching against the full path of the
    /// type.
    ///
    /// In SQL terms, this will execute a similar query to the following:
    ///
    /// ```ignore,sql
    /// SELECT * FROM searchIndex WHERE name LIKE ? AND type IN (?)
    /// ```
    ///
    /// Note that matching is case-insensitive.
    ///
    /// For example, if you search for `Value` in the `serde_json` crate,
    /// assuming the default `types` parameter, then this query will match any
    /// types with `Value` in their path, including methods such as
    /// `Value::is_object`.
    pub query: String,

    /// Optional filter to search for specific item types.
    #[serde(default = "EntryType::all")]
    pub kinds: Vec<EntryType>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CrateResourceRequest {
    /// Crate resource URI.
    pub uri: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CrateVersionsRequest {
    /// The exact name of the crate.
    pub crate_name: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CrateReadmeRequest {
    /// The exact name of the crate.
    pub crate_name: String,

    /// The version of the crate.
    /// Either a semantic version or `latest` for the latest published crate
    /// version.
    #[serde(default = "default_latest")]
    pub crate_version: String,
}

fn default_latest() -> String {
    "latest".to_string()
}

#[tool_router]
impl BookwormService {
    #[must_use]
    pub fn new(config: ServerConfig) -> Self {
        Self {
            config,
            tool_router: Self::tool_router(),
        }
    }

    #[tool(
        name = "crates_search",
        description = "Search for crates matching the given query.\n\nThe returned list contains \
                       a list of URIs for each crate to fetch additional crate information."
    )]
    async fn crates_search(
        &self,
        Parameters(req): Parameters<SearchCratesRequest>,
    ) -> Result<CallToolResult, McpError> {
        if req.query.is_empty() {
            return Err(McpError::invalid_params("query must not be empty", None));
        }

        match self.run_crates_search(&req.query).await {
            Ok(contents) => Ok(self.format_contents(contents)),
            Err(err) => Ok(self.format_error(&err)),
        }
    }

    #[tool(
        name = "crate_search_items",
        description = "Search for item definitions within a crate.\n\nReturns a list of matching \
                       items including their path, type, signature, documentation, and related \
                       resource URIs."
    )]
    async fn crate_search_items(
        &self,
        Parameters(req): Parameters<SearchCrateItemsRequest>,
    ) -> Result<CallToolResult, McpError> {
        if req.crate_name.is_empty() {
            return Err(McpError::invalid_params(
                "crate_name must not be empty",
                None,
            ));
        }
        if req.query.is_empty() {
            return Err(McpError::invalid_params("query must not be empty", None));
        }
        if !valid_crate_version(&req.crate_version) {
            return Err(McpError::invalid_params(
                format!("invalid crate_version: {}", req.crate_version),
                None,
            ));
        }

        match self
            .run_crate_search_items(&req.crate_name, &req.crate_version, &req.query, req.kinds)
            .await
        {
            Ok(contents) => Ok(self.format_contents(contents)),
            Err(err) => Ok(self.format_error(&err)),
        }
    }

    #[tool(
        name = "crate_resource",
        description = "Get the resource for a crate.\n\nSupported URIs:\n- `crate://{crate_name}` \
                       - list crate versions\n- `crate://{crate_name}/{crate_version}` - get \
                       metadata\n- `crate://{crate_name}/{crate_version}/readme` - get readme \
                       content\n- `crate://{crate_name}/{crate_version}/items` - list item \
                       resources\n- `crate://{crate_name}/{crate_version}/src` - list source code \
                       resources\n- `crate://{crate_name}/{crate_version}/{path}` - get item/src \
                       resource"
    )]
    async fn crate_resource(
        &self,
        Parameters(req): Parameters<CrateResourceRequest>,
    ) -> Result<CallToolResult, McpError> {
        if req.uri.is_empty() {
            return Err(McpError::invalid_params("uri must not be empty", None));
        }

        let url = match Url::from_str(&req.uri) {
            Ok(url) => url,
            Err(err) => {
                return Err(McpError::invalid_params(
                    format!("invalid URI: {err}"),
                    None,
                ));
            }
        };
        let crate_uri = match CrateUri::try_from(&url) {
            Ok(uri) => uri,
            Err(err) => return Ok(self.format_error(&err)),
        };

        match self.run_crate_resource(&crate_uri).await {
            Ok(contents) => Ok(self.format_contents(contents)),
            Err(err) => Ok(self.format_error(&err)),
        }
    }

    #[tool(
        name = "crate_versions",
        description = "Get a list of most recent versions of a crate."
    )]
    async fn crate_versions(
        &self,
        Parameters(req): Parameters<CrateVersionsRequest>,
    ) -> Result<CallToolResult, McpError> {
        if req.crate_name.is_empty() {
            return Err(McpError::invalid_params(
                "crate_name must not be empty",
                None,
            ));
        }

        let uri = CrateUri::versions(&req.crate_name);
        match self.run_crate_resource(&uri).await {
            Ok(contents) => Ok(self.format_contents(contents)),
            Err(err) => Ok(self.format_error(&err)),
        }
    }

    #[tool(
        name = "crate_readme",
        description = "Get the README for a specific crate version."
    )]
    async fn crate_readme(
        &self,
        Parameters(req): Parameters<CrateReadmeRequest>,
    ) -> Result<CallToolResult, McpError> {
        if req.crate_name.is_empty() {
            return Err(McpError::invalid_params(
                "crate_name must not be empty",
                None,
            ));
        }
        if !valid_crate_version(&req.crate_version) {
            return Err(McpError::invalid_params(
                format!("invalid crate_version: {}", req.crate_version),
                None,
            ));
        }

        let uri = CrateUri::readme(&req.crate_name, &req.crate_version);
        match self.run_crate_resource(&uri).await {
            Ok(contents) => Ok(self.format_contents(contents)),
            Err(err) => Ok(self.format_error(&err)),
        }
    }
}

impl BookwormService {
    async fn run_crates_search(&self, query: &str) -> Result<Vec<Content>, Error> {
        debug!(query, "Searching for crates.");

        let crates = query::search_crates(query).await?;
        debug!(crates = crates.len(), "Found crates.");

        if crates.is_empty() {
            return Ok(vec![Content::text(
                "No crates found matching the query. Try partial words.",
            )]);
        }

        crates
            .into_iter()
            .map(|info| {
                Ok(Content::resource(ResourceContents::TextResourceContents {
                    uri: format!("crate://{}/{}/", info.name, info.version),
                    mime_type: None,
                    text: format_xml(&info, "Crate")?,
                    meta: None,
                }))
            })
            .collect()
    }

    async fn run_crate_search_items(
        &self,
        crate_name: &str,
        crate_version: &str,
        query: &str,
        kinds: Vec<EntryType>,
    ) -> Result<Vec<Content>, Error> {
        let definitions =
            query::search_crate_type_definitions(crate_name, crate_version, query, kinds, None)
                .await?;

        if definitions.is_empty() {
            return Ok(vec![Content::text(
                "No crate items found matching the query. Try broadening your search query.",
            )]);
        }

        let content = definitions
            .into_iter()
            .map(|info| {
                Ok(Content::resource(ResourceContents::TextResourceContents {
                    uri: info.docs_resource.clone(),
                    mime_type: None,
                    text: format_xml(&info, "Item")?,
                    meta: None,
                }))
            })
            .collect::<Result<Vec<_>, Error>>()?;

        Ok(truncate_resources(content))
    }

    async fn run_crate_resource(&self, uri: &CrateUri) -> Result<Vec<Content>, Error> {
        let Some(version) = &uri.version else {
            return versions_handler(&uri.name).await;
        };

        let Some(root) = &uri.root else {
            return metadata_handler(&uri.name, version).await;
        };

        match root {
            PathRoot::Readme => readme_handler(&uri.name, version).await,
            PathRoot::Items if uri.path.as_os_str().is_empty() => {
                list_items_handler(&uri.name, version).await
            }
            PathRoot::Items => item_resource_handler(uri).await,
            PathRoot::Src if uri.path.as_os_str().is_empty() => {
                list_src_handler(&uri.name, version).await
            }
            PathRoot::Src => src_resource_handler(uri).await,
        }
    }
}

async fn versions_handler(crate_name: &str) -> Result<Vec<Content>, Error> {
    query::crate_versions(crate_name)
        .await?
        .into_iter()
        .filter_map(|v| {
            (!v.yanked).then(|| {
                format_xml(&v, "CrateVersion").map(|s| {
                    Content::embedded_text(CrateUri::metadata(crate_name, v.num.clone()), s)
                })
            })
        })
        .collect()
}

async fn metadata_handler(crate_name: &str, crate_version: &str) -> Result<Vec<Content>, Error> {
    let metadata = query::crate_metadata(crate_name, crate_version).await?;

    Ok(vec![Content::embedded_text(
        CrateUri::metadata(crate_name, crate_version),
        format_xml(&metadata, "CrateMetadata")?,
    )])
}

async fn readme_handler(crate_name: &str, crate_version: &str) -> Result<Vec<Content>, Error> {
    // Crates.io does not support "latest"; resolve to the most recent version.
    let crate_version = if crate_version == "latest" {
        query::crate_versions(crate_name)
            .await?
            .into_iter()
            .next()
            .ok_or(Error::VersionNotFound {
                crate_name: crate_name.to_string(),
                version: crate_version.to_string(),
            })?
            .num
    } else {
        crate_version.to_owned()
    };

    let readme = query::crate_readme(crate_name, &crate_version).await?;
    Ok(vec![Content::embedded_text(
        CrateUri::readme(crate_name, crate_version),
        readme,
    )])
}

async fn list_items_handler(crate_name: &str, crate_version: &str) -> Result<Vec<Content>, Error> {
    // Listings drop the heavier `documentation` HTML so the response stays
    // small; clients fetch full docs per-item via `docs_resource`. The
    // remaining fields (path, kind, type_info, URIs) are enough for the LLM
    // to decide which items to drill into.
    let content = query::search_crate_type_definitions(crate_name, crate_version, "", vec![], None)
        .await?
        .into_iter()
        .map(|mut t| {
            t.item.documentation = None;
            let uri = t.docs_resource.clone();
            Ok(Content::resource(ResourceContents::TextResourceContents {
                uri,
                mime_type: None,
                text: format_xml(&t, "Item")?,
                meta: None,
            }))
        })
        .collect::<Result<Vec<_>, Error>>()?;

    Ok(truncate_resources(content))
}

async fn list_src_handler(crate_name: &str, crate_version: &str) -> Result<Vec<Content>, Error> {
    let uris = query::list_crate_source_resources(crate_name, Some(crate_version)).await?;

    Ok(vec![Content::embedded_text(
        CrateUri::src(crate_name, crate_version),
        format_xml(&uris, "Resources")?,
    )])
}

async fn item_resource_handler(uri: &CrateUri) -> Result<Vec<Content>, Error> {
    let item = query::get_crate_item_resource(&uri.into()).await?;
    Ok(vec![Content::embedded_text(
        uri.to_string(),
        format_xml(&item, "Item")?,
    )])
}

async fn src_resource_handler(uri: &CrateUri) -> Result<Vec<Content>, Error> {
    let src = query::get_crate_source_resource(&uri.into()).await?;
    Ok(vec![Content::embedded_text(uri.to_string(), src)])
}

#[tool_handler(router = self.tool_router)]
impl rmcp::ServerHandler for BookwormService {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("bookworm", env!("CARGO_PKG_VERSION")))
            .with_instructions(indoc! {r#"
                The "bookworm" server provides access to Rust crate type definitions,
                documentation and source code.
            "#})
    }
}
