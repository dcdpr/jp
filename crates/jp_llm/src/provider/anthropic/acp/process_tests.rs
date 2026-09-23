use std::ffi::OsStr;

use tokio::io::{AsyncBufReadExt as _, BufReader};

use super::*;

#[test]
fn adapter_launcher_uses_the_platform_entry_point() {
    let command = command(None);
    #[cfg(windows)]
    {
        assert_eq!(command.as_std().get_program(), "cmd.exe");
        assert_eq!(command.as_std().get_args().collect::<Vec<_>>(), [
            "/D",
            "/C",
            "claude-agent-acp.cmd"
        ]);
    }
    #[cfg(unix)]
    assert_eq!(command.as_std().get_program(), "claude-agent-acp");
}

#[test]
fn named_login_sets_both_configuration_and_credential_locations() {
    let first = command(Some(Utf8Path::new("/accounts/sub")));
    let second = command(Some(Utf8Path::new("/accounts/sub2")));
    assert_eq!(
        first
            .as_std()
            .get_envs()
            .find(|(key, _)| *key == "CLAUDE_CONFIG_DIR"),
        Some((
            OsStr::new("CLAUDE_CONFIG_DIR"),
            Some(OsStr::new("/accounts/sub"))
        ))
    );
    assert_eq!(
        second
            .as_std()
            .get_envs()
            .find(|(key, _)| *key == "CLAUDE_CONFIG_DIR"),
        Some((
            OsStr::new("CLAUDE_CONFIG_DIR"),
            Some(OsStr::new("/accounts/sub2"))
        ))
    );
    assert_eq!(
        second
            .as_std()
            .get_envs()
            .find(|(key, _)| *key == "CLAUDE_SECURESTORAGE_CONFIG_DIR"),
        Some((
            OsStr::new("CLAUDE_SECURESTORAGE_CONFIG_DIR"),
            Some(OsStr::new("/accounts/sub2"))
        ))
    );
}

#[test]
fn unnamed_login_preserves_inherited_configuration_and_credential_locations() {
    let command = command(None);
    assert_eq!(
        command
            .as_std()
            .get_envs()
            .find(|(key, _)| *key == "CLAUDE_CONFIG_DIR"),
        None
    );
    assert_eq!(
        command
            .as_std()
            .get_envs()
            .find(|(key, _)| *key == "CLAUDE_SECURESTORAGE_CONFIG_DIR"),
        None
    );
    assert_eq!(login_environment(None), BTreeMap::new());
}

#[tokio::test]
async fn process_failure_retains_stderr() {
    #[cfg(unix)]
    let mut command = Command::new("sh");
    #[cfg(unix)]
    command.args(["-c", "printf fixture-error >&2; exit 7"]);
    #[cfg(windows)]
    let mut command = Command::new("cmd.exe");
    #[cfg(windows)]
    command.args(["/D", "/C", "echo fixture-error >&2 & exit /B 7"]);
    // The adapter never speaks: the foreground request cannot be answered, so
    // the exit status is what ends the connection.
    let error = tokio::time::timeout(
        Duration::from_secs(5),
        run(
            command,
            Tap::none(),
            Box::new(|_| Box::pin(async { Ok(serde_json::Value::Null) })),
            |_peer| async { Ok(()) },
        ),
    )
    .await
    .unwrap()
    .unwrap_err();
    // Shell line endings differ by OS.
    let detail = error.data.unwrap();
    assert_eq!(detail["cause"]["exit_code"], 7);
    assert_eq!(detail["stderr"].as_str().unwrap().trim(), "fixture-error");
}

#[tokio::test]
async fn dropping_the_process_closes_its_open_output_pipe() {
    #[cfg(unix)]
    let mut command = Command::new("sh");
    #[cfg(unix)]
    command.args(["-c", "sh -c 'echo ready; exec sleep 60' & wait"]);
    #[cfg(windows)]
    let mut command = Command::new("cmd.exe");
    #[cfg(windows)]
    command.args(["/D", "/C", "echo ready & set /P pending="]);
    command.stdin(Stdio::piped()).stdout(Stdio::piped());
    let mut child = spawn(command).unwrap();
    let stdin = child.stdin().take().unwrap();
    let mut stdout = BufReader::new(child.stdout().take().unwrap());
    let mut ready = String::new();
    tokio::time::timeout(Duration::from_secs(5), stdout.read_line(&mut ready))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(ready.trim(), "ready");
    assert!(child.try_wait().unwrap().is_none());
    drop(child);
    let mut rest = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), stdout.read_to_end(&mut rest))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(rest, b"");
    // Retain stdin until after EOF so closing it cannot make the fixture exit.
    drop(stdin);
}

#[tokio::test]
async fn stderr_capture_drains_but_keeps_only_the_tail() {
    let input = vec![b'x'; 70000];
    let output = stderr_tail(input.as_slice()).await.unwrap();
    assert_eq!(output, vec![b'x'; 65536]);
}
