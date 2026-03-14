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
fn test_uri_to_command_hierarchical() {
    let cases = [
        (
            "cmd://ls",
            Ok(Command {
                cmd: "ls".to_string(),
                args: vec![],
                description: None,
            }),
        ),
        (
            "cmd://ls?description=hello%20world",
            Ok(Command {
                cmd: "ls".to_string(),
                args: vec![],
                description: Some("hello world".to_string()),
            }),
        ),
        (
            "cmd://ls?arg=%2Dlah",
            Ok(Command {
                cmd: "ls".to_string(),
                args: vec!["-lah".to_string()],
                description: None,
            }),
        ),
        (
            "cmd://ls?arg=%2Dlah&description=hello%20world",
            Ok(Command {
                cmd: "ls".to_string(),
                args: vec!["-lah".to_string()],
                description: Some("hello world".to_string()),
            }),
        ),
        (
            "cmd://git?arg=diff&arg=%2D%2Dcached",
            Ok(Command {
                cmd: "git".to_string(),
                args: vec!["diff".to_string(), "--cached".to_string()],
                description: None,
            }),
        ),
        (
            "cmd://ls?arg=%2Dl&arg=%2Da&arg=%2Dh",
            Ok(Command {
                cmd: "ls".to_string(),
                args: vec!["-l".to_string(), "-a".to_string(), "-h".to_string()],
                description: None,
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

#[test]
fn test_uri_to_command_opaque() {
    let cases = [
        (
            "cmd:ls",
            Ok(Command {
                cmd: "ls".to_string(),
                args: vec![],
                description: None,
            }),
        ),
        (
            "cmd:git diff --cached",
            Ok(Command {
                cmd: "git".to_string(),
                args: vec!["diff".to_string(), "--cached".to_string()],
                description: None,
            }),
        ),
        (
            "cmd:ls -l -a -h",
            Ok(Command {
                cmd: "ls".to_string(),
                args: vec!["-l".to_string(), "-a".to_string(), "-h".to_string()],
                description: None,
            }),
        ),
        (
            "cmd:git commit -m 'hello world'",
            Ok(Command {
                cmd: "git".to_string(),
                args: vec![
                    "commit".to_string(),
                    "-m".to_string(),
                    "hello world".to_string(),
                ],
                description: None,
            }),
        ),
        (
            "cmd:git diff --cached?description=staged changes",
            Ok(Command {
                cmd: "git".to_string(),
                args: vec!["diff".to_string(), "--cached".to_string()],
                description: Some("staged changes".to_string()),
            }),
        ),
        ("cmd:", Err("Empty command")),
    ];

    for (uri, expected) in cases {
        let uri = Url::parse(uri).unwrap();
        let command = uri_to_command(&uri).map_err(|e| e.to_string());
        assert_eq!(command, expected.map_err(str::to_string));
    }
}

#[tokio::test]
async fn test_commands_get() {
    let commands = Commands(
        vec![
            Command {
                cmd: "ls".to_owned(),
                args: vec![],
                description: None,
            },
            Command {
                cmd: "ls".to_owned(),
                args: vec!["-a".to_owned()],
                description: None,
            },
            Command {
                cmd: "false".to_owned(),
                args: vec![],
                description: Some("Run false".to_owned()),
            },
        ]
        .into_iter()
        .collect(),
    );

    let root = camino_tempfile::tempdir().unwrap();
    let path = root.path();
    std::fs::create_dir_all(path.join("dir")).unwrap();
    std::fs::write(path.join("file1"), "").unwrap();
    std::fs::write(path.join("file2"), "").unwrap();

    let client = Client::new(IndexMap::default());
    let attachments = commands.get(path, client).await.unwrap();
    assert_eq!(attachments, vec![
        Attachment::text("false", indoc::indoc! {"
                    <Output>
                      <code>1</code>
                    </Output>"},)
        .with_description("Run false"),
        Attachment::text("ls", indoc::indoc! {"
                    <Output>
                      <stdout>dir\nfile1\nfile2\n</stdout>
                      <code>0</code>
                    </Output>"},),
        Attachment::text("ls -a", indoc::indoc! {"
                    <Output>
                      <stdout>.\n..\ndir\nfile1\nfile2\n</stdout>
                      <code>0</code>
                    </Output>"},),
    ]);
}
