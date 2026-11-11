//! `jp vite build`: build the site for publishing, as `vitepress build` does,
//! with this binary rendering its release pages.
//!
//! Before starting `vitepress build`, build brings the settings snapshot up to
//! date, as `serve` does, and refuses while a file `jp vite sync` writes isn't
//! the one this binary writes, since the site would be built with loaders from
//! another release.
//! It names this binary to the site's loaders in [`BIN`], so the release pages
//! need no `changelog` on `PATH`.
//! Nothing else differs from a `vitepress build`: the pages show no drafts, and
//! accept no edits.
//!
//! [`BIN`]: crate::serve::BIN

use std::{
    env,
    io::{BufRead, Write},
    sync::mpsc::{self, Receiver, RecvTimeoutError},
    thread::JoinHandle,
    time::{Duration, Instant},
};

use camino::Utf8Path;
use jp_plugin::message::HostToPlugin;
use serde_json::Value;

use crate::{
    git,
    options::Options,
    print_to,
    serve::{self, Event, Revisions, TICK},
    settings::{snapshot, source_dir},
    site, sync,
};

/// How long the output of a finished build is waited for, once `vitepress` has
/// exited.
const LINGER: Duration = Duration::from_secs(2);

/// Build the site of the project at `root`, with `vitepress` the arguments
/// `vitepress build` gets.
///
/// `stdin` goes to a thread of its own, so the host's shutdown stops the build;
/// everything sent to the host goes through `stdout` from here.
/// The error says why the site can't be built, or that `vitepress build` failed
/// or was stopped.
pub(crate) fn build<R, W>(
    stdin: R,
    stdout: &mut W,
    root: &Utf8Path,
    options: &Options,
    config: &Value,
    vitepress: &[String],
) -> Result<(), String>
where
    R: BufRead + Send + 'static,
    W: Write,
{
    let site_dir = source_dir(root, options)?;
    if let Some(reason) = serve::unready(root, &site_dir) {
        return Err(reason);
    }

    let settings = snapshot(root, options, config, &git::remote(root))?;
    if let Some(note) = serve::write_snapshot(root, &settings, None)? {
        print_to(stdout, &note, "chrome")?;
    }

    // The plugins' site revisions matter to edits, which a build makes none of:
    // only the files the loaders run from are checked.
    let changes = sync::plan(root, site::owned(&settings), &site::owned_dir(&settings))?;
    let unchecked = Revisions {
        rfd: None,
        ticket: None,
        wiki: None,
    };
    if let Some(problem) = serve::problems(&changes, &site::settings_path(&settings), &unchecked) {
        return Err(problem);
    }

    let renderer = env::current_exe().ok();
    let mut child = serve::vitepress_command(&site_dir, "build", vitepress, renderer.as_deref())
        .spawn()
        .map_err(|error| {
            format!(
                "`{} build` couldn't be started: {error}",
                serve::runner(&site_dir).join(" ")
            )
        })?;

    let (events, received) = mpsc::channel();
    let mut forwarders = vec![];
    if let Some(output) = child.stdout.take() {
        forwarders.push(serve::forward(output, "content", events.clone()));
    }
    if let Some(output) = child.stderr.take() {
        forwarders.push(serve::forward(output, "chrome", events.clone()));
    }
    serve::listen(stdin, events);

    loop {
        if let Some(status) = child
            .try_wait()
            .map_err(|error| format!("`vitepress build`: {error}"))?
        {
            // A failed build says why last, so its output is read to the end.
            print_rest(stdout, &received, &forwarders)?;

            return if status.success() {
                Ok(())
            } else {
                Err(format!("`vitepress build` failed: {status}."))
            };
        }

        match received.recv_timeout(TICK) {
            Ok(Event::Output { text, channel }) => print_to(stdout, &text, channel)?,
            Ok(Event::Host(Ok(HostToPlugin::Shutdown) | Err(_)))
            | Err(RecvTimeoutError::Disconnected) => {
                serve::stop(&mut child);
                return Err("The build was stopped before it finished.".to_owned());
            }
            Ok(Event::Host(Ok(_))) | Err(RecvTimeoutError::Timeout) => {}
        }
    }
}

/// Print what `vitepress` printed that hasn't reached the host yet, waiting up
/// to [`LINGER`] for `forwarders` to reach the end of its output.
fn print_rest(
    stdout: &mut impl Write,
    received: &Receiver<Event>,
    forwarders: &[JoinHandle<()>],
) -> Result<(), String> {
    let deadline = Instant::now() + LINGER;
    while forwarders.iter().any(|forwarder| !forwarder.is_finished()) && Instant::now() < deadline {
        if let Ok(Event::Output { text, channel }) = received.recv_timeout(TICK) {
            print_to(stdout, &text, channel)?;
        }
    }

    while let Ok(event) = received.try_recv() {
        if let Event::Output { text, channel } = event {
            print_to(stdout, &text, channel)?;
        }
    }

    Ok(())
}

#[cfg(test)]
#[path = "build_tests.rs"]
mod tests;
