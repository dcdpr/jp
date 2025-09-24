use std::{collections::BTreeSet, error::Error, path::Path};

use async_trait::async_trait;
use jp_attachment::{
    distributed_slice, linkme, percent_decode_str, percent_encode_str, typetag, Attachment,
    BoxedHandler, Handler, HANDLERS,
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
}

impl Command {
    fn to_uri(&self, scheme: &str) -> Result<Url, Box<dyn std::error::Error + Send + Sync>> {
        let query_pairs = self
            .args
            .iter()
            .map(|v| format!("arg={}", percent_encode_str(v)))
            .collect::<Vec<_>>()
            .join("&");

        let mut uri = format!("{scheme}://{}", &self.cmd);
        if !query_pairs.is_empty() {
            uri.push_str(&format!("?{query_pairs}"));
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

    async fn add(&mut self, uri: &Url) -> Result<(), Box<dyn Error + Send + Sync>> {
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
        root: &Path,
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

            attachments.push(Attachment {
                source: command.to_uri(self.scheme())?.to_string(),
                content: output.try_to_xml()?,
            });
        }

        Ok(attachments)
    }
}

fn uri_to_command(uri: &Url) -> Result<Command, Box<dyn Error + Send + Sync>> {
    let cmd = uri.host_str().ok_or("Invalid command URI")?;
    let args = uri
        .query_pairs()
        .filter_map(|(k, v)| (k == "arg").then(|| v.to_string()))
        .map(|v| percent_decode_str(&v))
        .collect::<Result<Vec<_>, _>>()?;

    Ok(Command {
        cmd: cmd.to_string(),
        args,
    })
}

#[cfg(test)]
mod tests {
    use indexmap::IndexMap;
    use test_log::test;

    use super::*;

    #[test]
    fn test_output_try_to_xml() {
        let output = Output {
            stdout: Some("Testing output".to_string()),
            stderr: None,
            code: 0,
        };

        let xml = output.try_to_xml().unwrap();
        assert_eq!(xml, indoc::indoc! {"
            <Output>
              <stdout>Testing output</stdout>
              <code>0</code>
            </Output>"});
    }

    #[test]
    fn test_uri_to_command_to_uri() {
        let cases = [
            (
                "cmd://ls",
                Ok(Command {
                    cmd: "ls".to_string(),
                    args: vec![],
                }),
            ),
            (
                "cmd://ls?arg=%2Dlah",
                Ok(Command {
                    cmd: "ls".to_string(),
                    args: vec!["-lah".to_string()],
                }),
            ),
            (
                "cmd://git?arg=diff&arg=%2D%2Dcached",
                Ok(Command {
                    cmd: "git".to_string(),
                    args: vec!["diff".to_string(), "--cached".to_string()],
                }),
            ),
            (
                "cmd://ls?arg=%2Dl&arg=%2Da&arg=%2Dh",
                Ok(Command {
                    cmd: "ls".to_string(),
                    args: vec!["-l".to_string(), "-a".to_string(), "-h".to_string()],
                }),
            ),
            (
                "cmd://?arg=%2Dl&arg=%2Da&arg=%2Dh",
                Err("Invalid command URI"),
            ),
        ];

        for (uri, expected) in cases {
            let uri = Url::parse(uri).unwrap();
            let command = uri_to_command(&uri).map_err(|e| e.to_string());
            assert_eq!(command, expected.map_err(str::to_string));

            if let Ok(command) = command {
                assert_eq!(command.to_uri("cmd").unwrap(), uri);
            }
        }
    }

    #[tokio::test]
    async fn test_commands_get() {
        let commands = Commands(
            vec![
                Command {
                    cmd: "ls".to_string(),
                    args: vec![],
                },
                Command {
                    cmd: "ls".to_string(),
                    args: vec!["-a".to_string()],
                },
                Command {
                    cmd: "false".to_string(),
                    args: vec![],
                },
            ]
            .into_iter()
            .collect(),
        );

        let root = tempfile::tempdir().unwrap();
        let path = root.path();
        std::fs::create_dir_all(path.join("dir")).unwrap();
        std::fs::write(path.join("file1"), "").unwrap();
        std::fs::write(path.join("file2"), "").unwrap();

        let client = Client::new(IndexMap::default());
        let attachments = commands.get(path, client).await.unwrap();
        assert_eq!(attachments, vec![
            Attachment {
                source: "cmd://false".to_string(),
                content: indoc::indoc! {"
                    <Output>
                      <code>1</code>
                    </Output>"}
                .to_owned(),
            },
            Attachment {
                source: "cmd://ls".to_string(),
                content: indoc::indoc! {"
                    <Output>
                      <stdout>dir\nfile1\nfile2\n</stdout>
                      <code>0</code>
                    </Output>"}
                .to_owned(),
            },
            Attachment {
                source: "cmd://ls?arg=%2Da".to_string(),
                content: indoc::indoc! {"
                    <Output>
                      <stdout>.\n..\ndir\nfile1\nfile2\n</stdout>
                      <code>0</code>
                    </Output>"}
                .to_owned(),
            },
        ]);
    }
}
