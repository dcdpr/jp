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
                env: vec![],
            }),
        ),
        (
            "cmd://ls?description=hello%20world",
            Ok(Command {
                cmd: "ls".to_string(),
                args: vec![],
                description: Some("hello world".to_string()),
                env: vec![],
            }),
        ),
        (
            "cmd://ls?arg=%2Dlah",
            Ok(Command {
                cmd: "ls".to_string(),
                args: vec!["-lah".to_string()],
                description: None,
                env: vec![],
            }),
        ),
        (
            "cmd://ls?arg=%2Dlah&description=hello%20world",
            Ok(Command {
                cmd: "ls".to_string(),
                args: vec!["-lah".to_string()],
                description: Some("hello world".to_string()),
                env: vec![],
            }),
        ),
        (
            "cmd://git?arg=diff&arg=%2D%2Dcached",
            Ok(Command {
                cmd: "git".to_string(),
                args: vec!["diff".to_string(), "--cached".to_string()],
                description: None,
                env: vec![],
            }),
        ),
        (
            "cmd://ls?arg=%2Dl&arg=%2Da&arg=%2Dh",
            Ok(Command {
                cmd: "ls".to_string(),
                args: vec!["-l".to_string(), "-a".to_string(), "-h".to_string()],
                description: None,
                env: vec![],
            }),
        ),
        (
            "cmd://?arg=%2Dl&arg=%2Da&arg=%2Dh",
            Err("Invalid command URI"),
        ),
        (
            "cmd://date?arg=%2B%25Y%2D%25m%2D%25d&description=Current%20Date",
            Ok(Command {
                cmd: "date".to_string(),
                args: vec!["+%Y-%m-%d".to_string()],
                description: Some("Current Date".to_string()),
                env: vec![],
            }),
        ),
        (
            "cmd://printenv?arg=TERM&env=TERM",
            Ok(Command {
                cmd: "printenv".to_string(),
                args: vec!["TERM".to_string()],
                description: None,
                env: vec!["TERM".to_string()],
            }),
        ),
        (
            "cmd://printenv?env=TERM&env=SHELL&description=hello%20world",
            Ok(Command {
                cmd: "printenv".to_string(),
                args: vec![],
                description: Some("hello world".to_string()),
                env: vec!["TERM".to_string(), "SHELL".to_string()],
            }),
        ),
        (
            "cmd://printenv?env=TERM=xterm",
            Err(
                "invalid `env` value `TERM=xterm`: expected the name of an environment variable, \
                 such as `TERM`",
            ),
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
                env: vec![],
            }),
        ),
        (
            "cmd:git diff --cached",
            Ok(Command {
                cmd: "git".to_string(),
                args: vec!["diff".to_string(), "--cached".to_string()],
                description: None,
                env: vec![],
            }),
        ),
        (
            "cmd:ls -l -a -h",
            Ok(Command {
                cmd: "ls".to_string(),
                args: vec!["-l".to_string(), "-a".to_string(), "-h".to_string()],
                description: None,
                env: vec![],
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
                env: vec![],
            }),
        ),
        (
            "cmd:git diff --cached?description=staged changes",
            Ok(Command {
                cmd: "git".to_string(),
                args: vec!["diff".to_string(), "--cached".to_string()],
                description: Some("staged changes".to_string()),
                env: vec![],
            }),
        ),
        (
            "cmd:printenv TERM?env=TERM",
            Ok(Command {
                cmd: "printenv".to_string(),
                args: vec!["TERM".to_string()],
                description: None,
                env: vec!["TERM".to_string()],
            }),
        ),
        (
            "cmd:printenv TERM?envs=TERM&envs=SHELL",
            Ok(Command {
                cmd: "printenv".to_string(),
                args: vec!["TERM".to_string()],
                description: None,
                env: vec!["TERM".to_string(), "SHELL".to_string()],
            }),
        ),
        (
            "cmd:printenv TERM?env[]=TERM&envs[]=SHELL",
            Ok(Command {
                cmd: "printenv".to_string(),
                args: vec!["TERM".to_string()],
                description: None,
                env: vec!["TERM".to_string(), "SHELL".to_string()],
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
async fn test_commands_get_missing_binary_names_command() {
    let commands = Commands(
        std::iter::once(Command {
            cmd: "jp-definitely-missing-binary".to_owned(),
            args: vec!["--check".to_owned()],
            description: None,
            env: vec![],
        })
        .collect(),
    );

    let root = camino_tempfile::tempdir().unwrap();
    let err = commands
        .get(root.path())
        .await
        .expect_err("spawning a missing binary should error");

    let msg = err.to_string();
    assert!(
        msg.contains("jp-definitely-missing-binary"),
        "error should name the failing command, got: {msg}"
    );
}

#[tokio::test]
async fn test_commands_get_passes_no_variable_the_command_does_not_name() {
    // Cargo sets `CARGO_PKG_NAME` in the test runner environment.
    assert_eq!(
        std::env::var("CARGO_PKG_NAME").as_deref(),
        Ok("jp_attachment_cmd_output"),
        "the variable must be set here for the command to be able to inherit it"
    );

    let commands = Commands(
        std::iter::once(Command {
            cmd: "sh".to_owned(),
            args: vec!["-c".to_owned(), "echo ${CARGO_PKG_NAME-unset}".to_owned()],
            description: None,
            env: vec![],
        })
        .collect(),
    );

    let root = camino_tempfile::tempdir().unwrap();
    let attachments = commands.get(root.path()).await.unwrap();

    assert_eq!(attachments, vec![Attachment::text(
        "sh -c echo ${CARGO_PKG_NAME-unset}",
        indoc::indoc! {"
            <Output>
              <stdout>unset\n</stdout>
              <code>0</code>
            </Output>"},
    )]);
}

#[tokio::test]
async fn test_commands_get_passes_a_variable_the_command_names() {
    let commands = Commands(
        std::iter::once(Command {
            cmd: "sh".to_owned(),
            args: vec!["-c".to_owned(), "echo ${CARGO_PKG_NAME-unset}".to_owned()],
            description: None,
            env: vec!["CARGO_PKG_NAME".to_owned()],
        })
        .collect(),
    );

    let root = camino_tempfile::tempdir().unwrap();
    let attachments = commands.get(root.path()).await.unwrap();

    assert_eq!(attachments, vec![Attachment::text(
        "sh -c echo ${CARGO_PKG_NAME-unset}",
        indoc::indoc! {"
            <Output>
              <stdout>jp_attachment_cmd_output\n</stdout>
              <code>0</code>
            </Output>"},
    )]);
}

#[tokio::test]
async fn test_commands_get() {
    let commands = Commands(
        vec![
            Command {
                cmd: "ls".to_owned(),
                args: vec![],
                description: None,
                env: vec![],
            },
            Command {
                cmd: "ls".to_owned(),
                args: vec!["-a".to_owned()],
                description: None,
                env: vec![],
            },
            Command {
                cmd: "false".to_owned(),
                args: vec![],
                description: Some("Run false".to_owned()),
                env: vec![],
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

    let attachments = commands.get(path).await.unwrap();
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
