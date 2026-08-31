//! Headless-browser fetch pipeline.
//!
//! Drives a locally installed Chrome or Chromium to load the URL, run its
//! scripts, and print the resulting DOM, which is then handed to the HTML
//! pipeline.
//! Use it for pages that assemble their content client-side, where a plain HTTP
//! fetch returns an empty shell.
//!
//! The browser binary is taken from `JP_HEADLESS_BROWSER` when set, and
//! otherwise from the first known executable name found on `PATH`.
//! Those are the only two ways it is located: a binary installed anywhere else
//! needs a symlink onto `PATH` or the environment variable.

use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Stdio,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

use tokio::{io::AsyncReadExt as _, process::Command, time::timeout};
use url::Url;

use super::{USER_AGENT_VALUE, html};
use crate::{
    Error,
    util::{ToolResult, error, truncate},
};

/// How far the page's timers are allowed to advance before the DOM is dumped.
/// This is virtual time, not wall clock: idle waits fast-forward, while real
/// network requests still take as long as they take.
const VIRTUAL_TIME_BUDGET_MS: u32 = 15_000;

/// Ceiling on how long the browser waits for the page before dumping what it
/// has.
///
/// This is a maximum, not a minimum: a page that finishes loading sooner is
/// captured sooner, so raising it costs nothing on pages that load normally and
/// only bounds ones that never finish.
const CAPTURE_TIMEOUT_MS: u32 = 20_000;

/// Backstop for a browser that ignores its own capture deadline.
const RENDER_TIMEOUT: Duration = Duration::from_secs(45);

/// Env var naming an explicit browser binary, overriding discovery.
const BROWSER_ENV: &str = "JP_HEADLESS_BROWSER";

/// Executable names to look for on `PATH`, in order of preference.
///
/// `chrome-headless-shell` comes first: it is a plain command-line binary that
/// dumps the DOM and exits, where a full Chrome runs new Headless mode, which
/// can hang without printing anything (crbug 327458826) and costs a render
/// timeout when it does.
const BROWSER_COMMANDS: &[&str] = &[
    "chrome-headless-shell",
    "google-chrome",
    "google-chrome-stable",
    "chromium",
    "chromium-browser",
    "chrome",
    "brave-browser",
    "microsoft-edge",
];

/// Distinguishes the profile directories of concurrent renders in one process.
static PROFILE_SEQ: AtomicU64 = AtomicU64::new(0);

pub(super) async fn fetch(
    url: &Url,
    list_sections: bool,
    sections: Option<Vec<String>>,
) -> ToolResult {
    // A browser loads `file:`, `data:` and `chrome:` URLs as readily as web
    // pages and `--dump-dom` prints whatever it finds, which would turn a tool
    // documented as HTTP(S)-only into a reader for any file the user can open.
    // The HTTP pipelines get this for free, since reqwest speaks no other
    // scheme.
    if !matches!(url.scheme(), "http" | "https") {
        return error(format!(
            "web_fetch reads `http` and `https` pages; `{}` URLs are not supported",
            url.scheme()
        ));
    }

    let Some(browser) = find_browser() else {
        return error(no_browser_message());
    };

    let body = render(&browser, url).await?;
    html::render(url, body, list_sections, sections).await
}

async fn render(browser: &Path, url: &Url) -> Result<String, Error> {
    // Chrome inside an app bundle never renders when handed a `--user-data-dir`:
    // it hangs until killed, whatever the directory and whether or not the
    // profile already exists, and produces no output even with `--screenshot`.
    // Its own default profile is separate from the everyday browser's, so
    // leaving the flag off costs no isolation.
    if is_app_bundle(browser) {
        return run_browser(browser, url, None).await;
    }

    let profile = temp_profile_dir();
    fs::create_dir_all(&profile)?;

    let result = run_browser(browser, url, Some(&profile)).await;

    // Best-effort: a leftover profile in the temp dir is harmless, and failing
    // the fetch over it would throw away a successful render.
    drop(fs::remove_dir_all(&profile));

    result
}

async fn run_browser(browser: &Path, url: &Url, profile: Option<&Path>) -> Result<String, Error> {
    let mut child = Command::new(browser)
        .args(browser_args(url, profile))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| format!("failed to start {}: {e}", browser.display()))?;

    let mut stdout = child.stdout.take().ok_or("browser stdout was not piped")?;
    let mut stderr = child.stderr.take().ok_or("browser stderr was not piped")?;
    let mut dom = String::new();
    let mut log = String::new();

    // Both pipes close once the browser has written everything it is going to.
    // Waiting on the pipes instead of on process exit means a browser that
    // hangs during shutdown (crbug 327583144) still yields its dump.
    let piped = timeout(RENDER_TIMEOUT, async {
        tokio::try_join!(
            stdout.read_to_string(&mut dom),
            stderr.read_to_string(&mut log),
        )
    })
    .await;

    // Kill and reap before the caller removes the profile directory:
    // `kill_on_drop` sends the signal synchronously but leaves reaping to the
    // runtime, so the browser can still hold files open once `drop` returns.
    // A child that already exited makes this a no-op.
    drop(child.kill().await);

    match piped {
        Err(_) => Err(render_error(
            &format!("timed out after {}s", RENDER_TIMEOUT.as_secs()),
            browser,
            &log,
        )),
        Ok(Err(e)) => Err(format!("failed to read output of {}: {e}", browser.display()).into()),
        Ok(Ok(_)) if dom.trim().is_empty() => {
            Err(render_error("produced an empty document", browser, &log))
        }
        Ok(Ok(_)) => Ok(dom),
    }
}

/// How to obtain a browser binary that renders reliably.
const INSTALL_HINT: &str = "For a binary that renders reliably, run `npx @puppeteer/browsers \
                            install chrome-headless-shell@stable --path <dir>`, then symlink it \
                            onto PATH or name it in JP_HEADLESS_BROWSER. It loads `icudtl.dat` \
                            from beside the executable, so symlink the unpacked binary rather \
                            than copying it out of its directory.";

/// Build a render failure message naming the binary, what it logged, and how to
/// get a working one.
fn render_error(what: &str, browser: &Path, log: &str) -> Error {
    let mut msg = format!("headless render {what} using {}", browser.display());

    // Chrome writes to stderr on successful runs too, so this is only worth
    // surfacing alongside a failure.
    let log = log.trim();
    if !log.is_empty() {
        msg.push_str(&format!(". Browser output: {}", truncate(log, 2_000)));
    }

    msg.push_str(&format!(". {INSTALL_HINT}"));
    msg.into()
}

/// True if `browser` is the executable inside a macOS `.app` bundle, rather
/// than a stand-alone command-line binary.
fn is_app_bundle(browser: &Path) -> bool {
    browser.components().any(|component| {
        Path::new(component.as_os_str())
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("app"))
    })
}

/// Build the browser command line.
///
/// `profile` isolates the run from any browser the user already has open, whose
/// profile lock would otherwise reject it.
/// Pass `None` for binaries that refuse to render with the flag set.
fn browser_args(url: &Url, profile: Option<&Path>) -> Vec<String> {
    let mut args = vec![
        "--headless".to_owned(),
        "--disable-gpu".to_owned(),
        "--dump-dom".to_owned(),
        // Chrome's default headless User-Agent carries a `HeadlessChrome`
        // token, which bot protection answers with a challenge page instead of
        // the content.
        format!("--user-agent={USER_AGENT_VALUE}"),
        format!("--virtual-time-budget={VIRTUAL_TIME_BUDGET_MS}"),
        format!("--timeout={CAPTURE_TIMEOUT_MS}"),
    ];

    if let Some(profile) = profile {
        args.push(format!("--user-data-dir={}", profile.display()));
        // A fresh profile directory otherwise puts Chrome through its
        // first-run flow.
        args.push("--no-first-run".to_owned());
        args.push("--no-default-browser-check".to_owned());
    }

    // Last, so nothing can read it as a flag value.
    args.push(url.to_string());
    args
}

fn find_browser() -> Option<PathBuf> {
    let found = match env::var_os(BROWSER_ENV).filter(|v| !v.is_empty()) {
        Some(path) => PathBuf::from(path),
        None => BROWSER_COMMANDS
            .iter()
            .find_map(|name| which::which(name).ok())?,
    };

    Some(resolve_binary(found))
}

/// Resolve symlinks so the browser starts from its own install directory.
///
/// Chrome locates `icudtl.dat`, its `.pak` files, and the V8 snapshot relative
/// to the executable path it was invoked with.
/// Started through a symlink in a `bin` directory it looks for them beside the
/// symlink, fails with `icudtl.dat not found in bundle`, and exits without
/// rendering.
///
/// Paths that can't be resolved are returned unchanged, so a bad path still
/// produces the browser's own error rather than one of ours.
fn resolve_binary(path: PathBuf) -> PathBuf {
    fs::canonicalize(&path).unwrap_or(path)
}

fn no_browser_message() -> String {
    format!(
        "No headless-capable browser found. Put one on PATH, or point {BROWSER_ENV} at the \
         binary. Searched PATH for: {}. {INSTALL_HINT}",
        BROWSER_COMMANDS.join(", ")
    )
}

fn temp_profile_dir() -> PathBuf {
    let seq = PROFILE_SEQ.fetch_add(1, Ordering::Relaxed);
    env::temp_dir().join(format!("jp-headless-{}-{seq}", std::process::id()))
}

#[cfg(test)]
#[path = "browser_tests.rs"]
mod tests;
