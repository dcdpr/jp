use std::{collections::BTreeSet, error::Error};

use async_trait::async_trait;
use camino::Utf8Path;
use jp_attachment::{
    Attachment, BoxedHandler, HANDLERS, Handler, distributed_slice, linkme, percent_decode_str,
    percent_encode_str, typetag,
};
use jp_mcp::Client;
use serde::{Deserialize, Serialize};
use url::Url;

#[distributed_slice(HANDLERS)]
#[linkme(crate = linkme)]
static HANDLER: fn() -> BoxedHandler = handler;

fn handler() -> BoxedHandler {
    (Box::new(Commands::default()) as Box<dyn Handler>).into()
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct Commands(BTreeSet<Command>);

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq, Hash, PartialOrd, Ord)]
struct Command {
    cmd: String,
    args: Vec<String>,
    description: Option<String>,
}

impl Command {
    fn to_uri(&self, scheme: &str) -> Result<Url, Box<dyn std::error::Error + Send + Sync>> {
        let mut query_pairs = self
            .args
            .iter()
            .map(|v| format!("arg={}", percent_encode_str(v)))
            .collect::<Vec<_>>();

        if let Some(prefix) = &self.description {
            query_pairs.push(format!("description={}", percent_encode_str(prefix)));
        }

        let mut uri = format!("{scheme}://{}", &self.cmd);
        if !query_pairs.is_empty() {
            uri.push_str(&format!("?{}", query_pairs.join("&")));
        }

        Ok(Url::parse(&uri)?)
    }
}

/// Output from a command.
#[derive(Debug, Clone, PartialEq, Serialize)]
struct Output {
    /// The standard output of the command.
    #[serde(skip_serializing_if = "Option::is_none")]
    stdout: Option<String>,

    /// The standard error of the command.
    #[serde(skip_serializing_if = "Option::is_none")]
    stderr: Option<String>,

    /// The exit code of the command.
    code: i32,
}

impl Output {
    pub fn try_to_xml(&self) -> Result<String, Box<dyn Error + Send + Sync>> {
        let mut buffer = String::new();
        let mut serializer = quick_xml::se::Serializer::new(&mut buffer);
        serializer.indent(' ', 2);
        self.serialize(serializer)?;
        Ok(buffer)
    }
}

#[typetag::serde(name = "cmd")]
#[async_trait]
impl Handler for Commands {
    fn scheme(&self) -> &'static str {
        "cmd"
    }

    async fn add(
        &mut self,
        uri: &Url,
        _cwd: &Utf8Path,
    ) -> Result<(), Box<dyn Error + Send + Sync>> {
        self.0.insert(uri_to_command(uri)?);

        Ok(())
    }

    async fn remove(&mut self, uri: &Url) -> Result<(), Box<dyn Error + Send + Sync>> {
        self.0.remove(&uri_to_command(uri)?);

        Ok(())
    }

    async fn list(&self) -> Result<Vec<Url>, Box<dyn Error + Send + Sync>> {
        let mut commands = vec![];
        for command in &self.0 {
            commands.push(command.to_uri(self.scheme())?);
        }

        Ok(commands)
    }

    async fn get(
        &self,
        root: &Utf8Path,
        _: Client,
    ) -> Result<Vec<Attachment>, Box<dyn Error + Send + Sync>> {
        let mut attachments = vec![];
        for command in &self.0 {
            let output = duct::cmd(command.cmd.as_str(), command.args.as_slice())
                .dir(root)
                .stdout_capture()
                .stderr_capture()
                .unchecked()
                .run()?;

            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);

            let output = Output {
                stdout: (!stdout.is_empty()).then(|| stdout.to_string()),
                stderr: (!stderr.is_empty()).then(|| stderr.to_string()),
                code: output.status.code().unwrap_or(0),
            };

            let mut attachment = Attachment::text(
                std::iter::once(command.cmd.clone())
                    .chain(command.args.iter().cloned())
                    .collect::<Vec<_>>()
                    .join(" "),
                output.try_to_xml()?,
            );
            attachment.description = command.description.clone();
            attachments.push(attachment);
        }

        Ok(attachments)
    }
}

fn uri_to_command(uri: &Url) -> Result<Command, Box<dyn Error + Send + Sync>> {
    if uri.cannot_be_a_base() {
        return parse_opaque_command(uri);
    }

    let cmd = uri.host_str().ok_or("Invalid command URI")?;
    let args = uri
        .query_pairs()
        .filter_map(|(k, v)| {
            (k == "arg" || k == "args" || k == "arg[]" || k == "args[]").then(|| v.to_string())
        })
        .map(|v| percent_decode_str(&v))
        .collect::<Result<Vec<_>, _>>()?;

    let description = uri
        .query_pairs()
        .find_map(|(k, v)| (k == "description").then(|| v.to_string()));

    Ok(Command {
        cmd: cmd.to_string(),
        args,
        description,
    })
}

/// Parse an opaque-path URL like `cmd:git diff --cached`.
///
/// The path is split using shell-word rules, so quoting works:
/// `cmd:git commit -m 'hello world'` produces `["git", "commit", "-m", "hello world"]`
fn parse_opaque_command(uri: &Url) -> Result<Command, Box<dyn Error + Send + Sync>> {
    let parts = shlex::split(uri.path()).ok_or("Invalid shell quoting in command")?;
    let (cmd, args) = parts.split_first().ok_or("Empty command")?;

    let description = uri
        .query_pairs()
        .find_map(|(k, v)| (k == "description").then(|| v.to_string()));

    Ok(Command {
        cmd: cmd.clone(),
        args: args.to_vec(),
        description,
    })
}

#[cfg(test)]
#[path = "lib_tests.rs"]
mod tests;
