//! Entry point for the `jp-vite` command plugin binary.
//!
//! When invoked as `jp-vite changelog ...`, dispatches directly to
//! [`changelog::cli::run`] so VitePress release data loaders can render release
//! pages under `jp vite serve` and `jp vite build` without a separate
//! `changelog` binary on `PATH`.

use std::{
    env,
    ffi::OsString,
    io::{self, BufReader, IsTerminal as _, Write},
    iter,
    process::ExitCode,
};

use jp_vite::{help_text, run};

fn main() -> ExitCode {
    let mut args = env::args_os().skip(1);
    if args.next().is_some_and(|first| first == "changelog") {
        return changelog::cli::run(iter::once(OsString::from("changelog")).chain(args));
    }

    // A human running the binary directly gets the help text rather than a
    // hung read on a protocol that is never going to speak.
    if io::stdin().is_terminal() {
        let mut err = io::stderr().lock();
        drop(writeln!(err, "{}", help_text()));
        drop(writeln!(err));
        drop(writeln!(
            err,
            "Note: this binary is a JP plugin. Run it via `jp vite`."
        ));
        return ExitCode::SUCCESS;
    }

    match run(BufReader::new(io::stdin()), io::stdout()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            let mut err = io::stderr().lock();
            drop(writeln!(err, "Fatal: {error}"));
            ExitCode::FAILURE
        }
    }
}
