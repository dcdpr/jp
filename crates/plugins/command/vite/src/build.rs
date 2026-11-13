//! `jp vite build`: running `vitepress build` with up-to-date site settings and
//! `JP_VITE_BIN` set to this binary for release-page rendering.
//!
//! Before spawning `vitepress build`, [`build`] refreshes the
//! `.vitepress/jp-vite/settings.json` snapshot and verifies that all `jp
//! vite`-owned files match the current binary (rejecting stale loaders from an
//! older release).

use std::{
    env,
    io::{BufRead, Write},
    sync::mpsc::{self, Receiver, RecvTimeoutError},
    thread::JoinHandle,
    time::{Duration, Instant},
};

use camino::{Utf8Path, Utf8PathBuf};
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

/// Maximum wait time to drain remaining stdout/stderr after `vitepress build`
/// exits.
const LINGER: Duration = Duration::from_secs(2);

/// Build the VitePress site for the workspace at `root`, passing `vitepress`
/// arguments to `vitepress build`.
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
    let site_dir = ready(stdout, root, options, config)?;

    run(stdin, stdout, &site_dir, vitepress)
}

/// Refresh the settings snapshot, verify that all `jp vite`-owned files are up
/// to date, and return the site's source directory.
fn ready(
    stdout: &mut impl Write,
    root: &Utf8Path,
    options: &Options,
    config: &Value,
) -> Result<Utf8PathBuf, String> {
    let site_dir = source_dir(root, options)?;
    if let Some(reason) = serve::unready(root, &site_dir) {
        return Err(reason);
    }

    let settings = snapshot(root, options, config, &git::remote(root))?;
    if let Some(note) = serve::write_snapshot(root, &settings, None)? {
        print_to(stdout, &note, "chrome")?;
    }

    let changes = sync::plan(root, site::owned(&settings), &site::owned_dir(&settings))?;
    let unchecked = Revisions {
        rfd: None,
        ticket: None,
        wiki: None,
    };
    if let Some(problem) = serve::problems(&changes, &site::settings_path(&settings), &unchecked) {
        return Err(problem);
    }

    Ok(site_dir)
}

fn run<R, W>(
    stdin: R,
    stdout: &mut W,
    site_dir: &Utf8Path,
    vitepress: &[String],
) -> Result<(), String>
where
    R: BufRead + Send + 'static,
    W: Write,
{
    let renderer = env::current_exe().ok();
    let mut child = serve::vitepress_command(site_dir, "build", vitepress, renderer.as_deref())
        .spawn()
        .map_err(|error| {
            format!(
                "`{} build` couldn't be started: {error}",
                serve::runner(site_dir).join(" ")
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
