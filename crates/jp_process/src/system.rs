//! The runner that spawns real processes.

use std::{
    io::{self, BufRead as _, BufReader, Read, Write as _},
    process::{Child, Command, ExitStatus, Stdio},
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, RecvTimeoutError, Sender},
    },
    thread,
    time::Duration,
};

use tokio_util::sync::CancellationToken;

use crate::{
    Ended, ExitCode, Finished, LineMatch, LineSink, ProcessOutput, ProcessRunner, ProcessSpec,
    Watch,
};

/// How often a watched run checks whether it was asked to stop.
const POLL: Duration = Duration::from_millis(5);

/// Spawns the process `spec` describes, as a child of this one.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemProcessRunner;

impl ProcessRunner for SystemProcessRunner {
    fn execute(&self, spec: &ProcessSpec, watch: &Watch) -> io::Result<Finished> {
        let mut command = Command::new(&spec.program);
        command
            .args(&spec.args)
            .current_dir(&spec.dir)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if spec.stdin.is_some() {
            command.stdin(Stdio::piped());
        }
        if spec.clean_env {
            command.env_clear();
        }
        command.envs(spec.env.iter().map(|(key, value)| (key, value)));

        #[cfg(unix)]
        if spec.own_process_group {
            use std::os::unix::process::CommandExt as _;
            command.process_group(0);
        }

        let mut child = command.spawn()?;

        if let (Some(input), Some(mut pipe)) = (spec.stdin.clone(), child.stdin.take()) {
            // Written from a thread of its own, so a process that prints before
            // it has read all its input cannot deadlock against this one.
            thread::spawn(move || drop(pipe.write_all(input.as_bytes())));
        }

        let stopped = Arc::new(AtomicBool::new(false));
        let (done, streams_closed) = mpsc::channel();
        let stdout = read(
            child.stdout.take(),
            None,
            watch.stop_when.clone(),
            &stopped,
            done.clone(),
        );
        let stderr = read(
            child.stderr.take(),
            watch.stderr_lines.clone(),
            watch.stop_when.clone(),
            &stopped,
            done,
        );

        let (status, ended) = if watch.cancellation.is_none() && watch.stop_when.is_none() {
            // Nothing can stop the run early, so there is nothing to check for.
            let _ = streams_closed.recv();
            let _ = streams_closed.recv();
            (child.wait()?, Ended::Exited)
        } else {
            supervise(&mut child, watch, &stopped, &streams_closed)?
        };

        Ok(Finished {
            output: ProcessOutput {
                stdout: collected(&stdout),
                stderr: collected(&stderr),
                status: ExitCode::from(status),
            },
            ended,
        })
    }
}

/// Wait for `child` to exit and its streams to close, stopping it if `watch`
/// asks to.
fn supervise(
    child: &mut Child,
    watch: &Watch,
    stopped: &AtomicBool,
    streams_closed: &mpsc::Receiver<()>,
) -> io::Result<(ExitStatus, Ended)> {
    let cancelled = || {
        watch
            .cancellation
            .as_ref()
            .is_some_and(CancellationToken::is_cancelled)
    };

    let mut status = None;
    let mut open = 2;
    loop {
        let ended = if cancelled() {
            Ended::Cancelled
        } else if stopped.load(Ordering::Acquire) {
            Ended::Stopped
        } else {
            if status.is_none() {
                status = child.try_wait()?;
            }
            if let Some(status) = status
                && open == 0
            {
                return Ok((status, Ended::Exited));
            }

            if open == 0 {
                thread::sleep(POLL);
            } else {
                match streams_closed.recv_timeout(POLL) {
                    Ok(()) => open -= 1,
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => open = 0,
                }
            }
            continue;
        };

        // A process that already exited only left its streams open to one it
        // started, which is not this runner's to stop.
        let status = match status {
            Some(status) => status,
            None => stop(child, watch.grace)?,
        };
        return Ok((status, ended));
    }
}

/// Stop `child`, giving it `grace` to exit after an interrupt before killing
/// it.
fn stop(child: &mut Child, grace: Duration) -> io::Result<ExitStatus> {
    #[cfg(unix)]
    if !grace.is_zero() {
        use std::time::Instant;

        interrupt(child.id());

        let deadline = Instant::now() + grace;
        while Instant::now() < deadline {
            if let Some(status) = child.try_wait()? {
                return Ok(status);
            }
            thread::sleep(POLL);
        }
    }

    // Windows has no interrupt to send, so there is nothing to wait for.
    #[cfg(not(unix))]
    let _ = grace;

    // Fails only for a process that already exited, which `wait` reports.
    drop(child.kill());
    child.wait()
}

/// Send `pid` the signal a Ctrl-C at the terminal would.
#[cfg(unix)]
fn interrupt(pid: u32) {
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return;
    };

    // SAFETY: `kill` takes plain integers and touches no memory. `pid` is a
    // child this process has not yet waited for, so the id cannot have been
    // reused by another process.
    unsafe {
        libc::kill(pid, libc::SIGINT);
    }
}

/// Read `pipe` to its end on a thread of its own, collecting what it carries.
///
/// Each line goes to `lines` and is tested against `stop_when`, which sets
/// `stopped` on a match.
/// `done` is told once the pipe has closed.
///
/// The thread is left to finish on its own: a process the runner stopped can
/// leave its streams open to one it started, which would otherwise hold the
/// caller until that one exits too.
fn read(
    pipe: Option<impl Read + Send + 'static>,
    lines: Option<LineSink>,
    stop_when: Option<LineMatch>,
    stopped: &Arc<AtomicBool>,
    done: Sender<()>,
) -> Arc<Mutex<Vec<u8>>> {
    let buffer = Arc::new(Mutex::new(Vec::new()));
    let Some(pipe) = pipe else {
        let _ = done.send(());
        return buffer;
    };

    let collected = Arc::clone(&buffer);
    let stopped = Arc::clone(stopped);
    thread::spawn(move || {
        let mut reader = BufReader::new(pipe);
        if lines.is_none() && stop_when.is_none() {
            let mut chunk = [0; 8192];
            while let Ok(read @ 1..) = reader.read(&mut chunk) {
                lock(&collected).extend_from_slice(&chunk[..read]);
            }
        } else {
            let mut line = Vec::new();
            loop {
                line.clear();
                if !matches!(reader.read_until(b'\n', &mut line), Ok(1..)) {
                    break;
                }
                lock(&collected).extend_from_slice(&line);

                let text = String::from_utf8_lossy(&line);
                let text = text.trim_end_matches(['\n', '\r']);
                if let Some(sink) = &lines {
                    sink(text);
                }
                if stop_when.as_ref().is_some_and(|stop| stop(text)) {
                    stopped.store(true, Ordering::Release);
                }
            }
        }
        let _ = done.send(());
    });

    buffer
}

/// What `buffer` has collected so far, decoded lossily.
fn collected(buffer: &Mutex<Vec<u8>>) -> String {
    String::from_utf8_lossy(&lock(buffer)).into_owned()
}

/// Nothing panics while holding a buffer's lock, so a poisoned one still holds
/// whole lines.
fn lock(buffer: &Mutex<Vec<u8>>) -> std::sync::MutexGuard<'_, Vec<u8>> {
    buffer.lock().unwrap_or_else(PoisonError::into_inner)
}
