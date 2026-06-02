//! agentic-shepherd attachment handler for `ag://` URIs.
//!
//! Resolves issues from the local `agentic-shepherd` tracker into a markdown
//! attachment by running the `agentic-shepherd` binary and rendering its JSON
//! output.
//!
//! Supported URI spellings (all referring to issue `592`):
//!
//! - `ag://issues/592`, `ag:issues/592`
//! - `ag://issue/592`, `ag:issue/592`
//! - `ag://592`, `ag:592` (bare number, defaults to the `issues` namespace)
//!
//! `issues` is the only namespace today; the grammar is shaped as
//! `ag://<namespace>/<id>` so more can be added without breaking existing
//! references.

use std::{collections::BTreeSet, error::Error};

use async_trait::async_trait;
use camino::Utf8Path;
use jp_attachment::{
    Attachment, BoxedHandler, HANDLERS, Handler, distributed_slice, linkme, typetag,
};
use jp_mcp::Client;
use serde::{Deserialize, Serialize};
use tracing::debug;
use url::Url;

use crate::{
    model::IssueDetail,
    uri::{Namespace, Reference},
};

mod model;
mod render;
mod uri;

#[distributed_slice(HANDLERS)]
#[linkme(crate = linkme)]
static HANDLER: fn() -> BoxedHandler = handler;

fn handler() -> BoxedHandler {
    (Box::new(AgenticShepherd::default()) as Box<dyn Handler>).into()
}

/// The binary invoked to resolve issues.
/// Must be available on `PATH`.
const BINARY: &str = "agentic-shepherd";

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct AgenticShepherd {
    references: BTreeSet<Reference>,
}

#[typetag::serde(name = "agentic_shepherd")]
#[async_trait]
impl Handler for AgenticShepherd {
    fn scheme(&self) -> &'static str {
        "ag"
    }

    async fn add(
        &mut self,
        uri: &Url,
        _cwd: &Utf8Path,
    ) -> Result<(), Box<dyn Error + Send + Sync>> {
        // Validate the URI shape now so a typo fails at attach time rather than
        // at the next conversation turn.
        self.references.insert(Reference::parse(uri)?);
        Ok(())
    }

    async fn remove(&mut self, uri: &Url) -> Result<(), Box<dyn Error + Send + Sync>> {
        self.references.remove(&Reference::parse(uri)?);
        Ok(())
    }

    async fn list(&self) -> Result<Vec<Url>, Box<dyn Error + Send + Sync>> {
        self.references.iter().map(Reference::to_url).collect()
    }

    async fn get(
        &self,
        root: &Utf8Path,
        _: Client,
    ) -> Result<Vec<Attachment>, Box<dyn Error + Send + Sync>> {
        debug!(
            count = self.references.len(),
            "Fetching agentic-shepherd attachments."
        );

        let mut attachments = Vec::with_capacity(self.references.len());
        for reference in &self.references {
            attachments.push(fetch(reference, root)?);
        }
        Ok(attachments)
    }
}

/// Run `agentic-shepherd` for a single reference and render its output.
fn fetch(
    reference: &Reference,
    root: &Utf8Path,
) -> Result<Attachment, Box<dyn Error + Send + Sync>> {
    let command = match reference.namespace() {
        Namespace::Issues => "IssueDetail",
    };
    let payload = serde_json::json!({
        "command": command,
        "issue_id_str": reference.id(),
    })
    .to_string();

    // A failed spawn yields a bare io::Error that doesn't name the binary, so
    // attach the name here while we still have it.
    let output = duct::cmd(BINARY, ["--json".to_string(), payload])
        .dir(root)
        .stdout_capture()
        .stderr_capture()
        .unchecked()
        .run()
        .map_err(|e| format!("failed to run `{BINARY}`: {e}"))?;

    if !output.status.success() {
        let code = output.status.code().unwrap_or(-1);
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("`{BINARY}` exited with status {code}: {}", stderr.trim()).into());
    }

    let detail: IssueDetail = serde_json::from_slice(&output.stdout)
        .map_err(|e| format!("failed to parse `{BINARY}` output as JSON: {e}"))?;

    let source = reference.to_url()?.to_string();
    let content = render::render(&detail);
    Ok(Attachment::text(source, content).with_description(detail.issue.title))
}

#[cfg(test)]
#[path = "lib_tests.rs"]
mod tests;
