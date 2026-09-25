//! A program for the runner's tests to spawn.
//!
//! Does what its arguments say, in order, so every test drives the same binary
//! on every platform instead of a shell script that only one of them can run:
//!
//! - `out:TEXT` / `err:TEXT`: print `TEXT` and a newline to stdout / stderr.
//! - `bytes`: print a byte that is not UTF-8 to stdout.
//! - `stdin`: copy stdin to stdout.
//! - `env:NAME`: print the variable's value, or `<unset>`.
//! - `cwd`: print the working directory.
//! - `group`: print the process id, then the process group id (unix only).
//! - `ignore-interrupt`: ignore Ctrl-C from here on (unix only).
//! - `interrupt-exits`: exit with code 42 on Ctrl-C from here on (unix only).
//! - `hold:MS`: start a copy of this program that sleeps `MS` milliseconds
//!   holding this one's stdout and stderr open, and leave it running.
//! - `sleep:MS`: sleep `MS` milliseconds.
//! - `exit:CODE`: exit with `CODE`.

use std::{
    env,
    io::{self, Read as _, Write},
    process::{Command, exit},
    thread,
    time::Duration,
};

fn main() {
    for arg in env::args().skip(1) {
        let (step, value) = arg.split_once(':').unwrap_or((&arg, ""));
        match step {
            "out" => line(&mut io::stdout(), value),
            "err" => line(&mut io::stderr(), value),
            "bytes" => drop(io::stdout().write_all(b"\xff\n")),
            "stdin" => {
                let mut input = Vec::new();
                drop(io::stdin().read_to_end(&mut input));
                drop(io::stdout().write_all(&input));
            }
            "env" => line(
                &mut io::stdout(),
                &env::var(value).unwrap_or_else(|_| "<unset>".to_owned()),
            ),
            "cwd" => line(
                &mut io::stdout(),
                &env::current_dir()
                    .map(|dir| dir.display().to_string())
                    .unwrap_or_default(),
            ),
            #[cfg(unix)]
            // SAFETY: both calls only read this process's own ids.
            "group" => line(&mut io::stdout(), &unsafe {
                format!("{} {}", libc::getpid(), libc::getpgrp())
            }),
            #[cfg(unix)]
            // SAFETY: installs a disposition, touching no memory of ours.
            "ignore-interrupt" => unsafe {
                libc::signal(libc::SIGINT, libc::SIG_IGN);
            },
            #[cfg(unix)]
            // SAFETY: the handler only calls `_exit`, which is async-signal-safe.
            "interrupt-exits" => unsafe {
                libc::signal(
                    libc::SIGINT,
                    exit_interrupted as *const () as libc::sighandler_t,
                );
            },
            "hold" => {
                if let Ok(program) = env::current_exe() {
                    drop(Command::new(program).arg(format!("sleep:{value}")).spawn());
                }
            }
            "sleep" => thread::sleep(Duration::from_millis(value.parse().unwrap_or(0))),
            "exit" => exit(value.parse().unwrap_or(0)),
            _ => {}
        }
    }
}

/// Exit with code 42, from a signal handler.
#[cfg(unix)]
extern "C" fn exit_interrupted(_signal: libc::c_int) {
    // SAFETY: `_exit` is async-signal-safe, and runs no destructors.
    unsafe { libc::_exit(42) }
}

/// Write `text` and a newline, flushed, so a reader sees it straight away.
fn line(out: &mut impl Write, text: &str) {
    drop(writeln!(out, "{text}"));
    drop(out.flush());
}
