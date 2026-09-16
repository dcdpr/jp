//! ACP process lifecycle using Unix process groups or Windows job objects.

use std::{
    collections::VecDeque,
    env, io,
    ops::{Deref, DerefMut},
    process::Stdio,
    time::Duration,
};

#[cfg(windows)]
use process_wrap::tokio::JobObject;
#[cfg(unix)]
use process_wrap::tokio::ProcessGroup;
use process_wrap::tokio::{ChildWrapper, CommandWrap, KillOnDrop};
use serde_json::json;
use tokio::{
    io::{AsyncRead, AsyncReadExt as _},
    process::Command,
};
use tracing::debug;

use super::{
    removes_variable,
    rpc::{self, Handler, Peer, RpcError, Tap},
};

pub(super) fn command() -> Command {
    #[cfg(windows)]
    let mut command = {
        // npm installs a .cmd shim on Windows. No prompt or model data is
        // interpolated here; those values travel over the ACP connection.
        let mut command = Command::new("cmd.exe");
        command.args(["/D", "/C", "claude-agent-acp.cmd"]);
        command
    };
    #[cfg(not(windows))]
    let mut command = Command::new("claude-agent-acp");
    for (name, _) in env::vars_os() {
        if name.to_str().is_some_and(removes_variable) {
            command.env_remove(name);
        }
    }
    command
}

pub(super) fn spawn(command: Command) -> io::Result<Child> {
    let mut command = CommandWrap::from(command);
    command.wrap(KillOnDrop);
    #[cfg(unix)]
    command.wrap(ProcessGroup::leader());
    #[cfg(windows)]
    command.wrap(JobObject);
    command.spawn().map(Child)
}

/// Drop uses the wrapper's termination method, not just Tokio's direct-child
/// kill-on-drop flag, so Unix descendants are terminated too.
pub(super) struct Child(Box<dyn ChildWrapper>);

impl Deref for Child {
    type Target = dyn ChildWrapper;
    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}

impl DerefMut for Child {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.0.as_mut()
    }
}

impl Drop for Child {
    fn drop(&mut self) {
        if let Err(error) = self.start_kill() {
            debug!(%error, "ACP child cleanup");
        }
    }
}

/// Spawn the adapter and run one ACP connection against its stdio.
///
/// `foreground` drives the request sequence; `handler` answers everything the
/// adapter initiates.
/// The connection ends when `foreground` returns, when the adapter exits, or
/// when either side fails.
///
/// A failure carries the tail of the adapter's stderr, which is usually the
/// only account of why a Node process died.
pub(super) async fn run<F, Fut>(
    command: Command,
    tap: Tap,
    handler: Handler,
    foreground: F,
) -> Result<(), RpcError>
where
    F: FnOnce(Peer) -> Fut,
    Fut: Future<Output = Result<(), RpcError>>,
{
    let mut command = command;
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = spawn(command).map_err(RpcError::into_internal_error)?;
    let stdin = child.stdin().take().expect("stdin is piped");
    let stdout = child.stdout().take().expect("stdout is piped");
    let stderr = child.stderr().take().expect("stderr is piped");
    let protocol = rpc::drive(stdin, stdout, tap, handler, foreground);
    tokio::pin!(protocol);
    let completion = async {
        let result = tokio::select! {
            result = &mut protocol => {
                match tokio::time::timeout(Duration::from_secs(1), child.wait()).await {
                    Ok(Ok(status)) if result.is_ok() && !status.success() => Err(RpcError::internal_error().data(json!({"exit_code":status.code(),"status":status.to_string()}))),
                    _ => result,
                }
            },
            status = child.wait() => match status {
                Ok(status) if status.success() => tokio::time::timeout(Duration::from_secs(1), &mut protocol).await
                    .map_err(RpcError::into_internal_error).and_then(|result| result),
                Ok(status) => Err(RpcError::internal_error().data(json!({"exit_code":status.code(),"status":status.to_string()}))),
                Err(error) => Err(RpcError::into_internal_error(error)),
            }
        };
        if let Err(error) = child.start_kill() {
            debug!(%error, "ACP process cleanup");
        }
        drop(tokio::time::timeout(Duration::from_secs(1), child.wait()).await);
        result
    };
    let (result, stderr) = tokio::join!(completion, stderr_tail(stderr));
    let stderr = stderr.map_err(RpcError::into_internal_error)?;
    result.map_err(|mut error| {
        if stderr.is_empty() {
            return error;
        }
        let cause = error.data.take();
        error.data(json!({"cause":cause,"stderr":String::from_utf8_lossy(&stderr)}))
    })
}

async fn stderr_tail(mut stderr: impl AsyncRead + Unpin) -> io::Result<Vec<u8>> {
    let mut tail = VecDeque::new();
    let mut buffer = [0; 8192];
    loop {
        let size = stderr.read(&mut buffer).await?;
        if size == 0 {
            return Ok(tail.into());
        }
        tail.extend(&buffer[..size]);
        let excess = tail.len().saturating_sub(65536);
        tail.drain(..excess);
    }
}

#[cfg(test)]
#[path = "process_tests.rs"]
mod tests;
