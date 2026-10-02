//! Plugin registry operations.
//!
//! Handles fetching, caching, and querying the plugin registry, as well as
//! downloading and installing plugin binaries.

use std::{
    io::Write as _,
    time::{Duration, SystemTime},
};

use camino::{Utf8Path, Utf8PathBuf};
use camino_tempfile::NamedUtf8TempFile;
use jp_plugin::registry::{Registry, RegistryBinary, RegistryPlugin};
use sha1::Sha1;
use sha2::{Digest, Sha256};
use tracing::{debug, warn};

use crate::cmd;

/// The URL of the official JP plugin registry.
const REGISTRY_URL: &str = "https://jp.computer/plugins.json";

/// Filename for the cached registry.
const REGISTRY_CACHE_FILE: &str = "registry.json";

/// Directory path for installed command plugin binaries.
const PLUGIN_DIR: &str = "plugins/command";

/// How old the cached registry may be before JP refreshes it on its own.
const CACHE_MAX_AGE: Duration = Duration::from_hours(24);

/// How long JP waits for the registry, whoever asked for it.
///
/// Short, because on a network that stalls rather than fails, the wait is the
/// whole cost: a mistyped command or `jp plugin list` should report what it
/// can, not hang.
/// The registry is one small file, so a working network answers well inside it.
const REGISTRY_TIMEOUT: Duration = Duration::from_secs(2);

/// How long JP waits to connect when it downloads a plugin binary.
///
/// Only the connection: a binary is several megabytes, and a total limit short
/// enough to catch a stalled network would cut a slow download short.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);

/// The HTTP client for the registry and plugin downloads.
pub(crate) fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(CONNECT_TIMEOUT)
        .build()
        .unwrap_or_default()
}

/// Whether JP may reach the network for plugins without being asked.
///
/// `JP_NO_PLUGIN_DOWNLOAD=1` turns off fetching the registry and downloading
/// official plugins on first use; `jp plugin install` and `jp plugin update`
/// still work, because running them asks for the network.
pub(crate) fn downloads_disabled() -> bool {
    std::env::var("JP_NO_PLUGIN_DOWNLOAD")
        .as_deref()
        .is_ok_and(|v| v == "1" || v == "true")
}

/// How old the cached registry is, or `None` when there is no cache.
fn cache_age() -> Option<Duration> {
    let modified = std::fs::metadata(cache_path()?).ok()?.modified().ok()?;

    // A cache stamped in the future is fresh as far as this is concerned.
    Some(
        SystemTime::now()
            .duration_since(modified)
            .unwrap_or_default(),
    )
}

/// When JP fetches the registry on its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Refresh {
    /// Only when there is no cache at all.
    WhenMissing,

    /// When there is no cache, or it is more than a day old.
    WhenStale,
}

/// Fetch and cache the registry, if `when` calls for it and downloads are
/// allowed.
///
/// Returns the fresh registry, or `None` when nothing was fetched: the cache
/// was fresh enough, downloads are off, or the fetch failed.
/// A failed fetch leaves the cache as it was.
pub(crate) async fn refresh(when: Refresh) -> Option<Registry> {
    if downloads_disabled() {
        return None;
    }

    let due = match (when, cache_age()) {
        (_, None) => true,
        (Refresh::WhenStale, Some(age)) => age > CACHE_MAX_AGE,
        (Refresh::WhenMissing, Some(_)) => false,
    };

    if !due {
        return None;
    }

    match fetch(&client()).await {
        Ok(registry) => Some(registry),
        Err(error) => {
            debug!(%error, "Could not refresh the plugin registry.");
            None
        }
    }
}

/// Path to the cached registry file.
pub(crate) fn cache_path() -> Option<Utf8PathBuf> {
    jp_workspace::user_data_dir()
        .ok()
        .map(|d| d.join(REGISTRY_CACHE_FILE))
}

/// Path to the directory where plugin binaries are installed.
pub(crate) fn bin_dir() -> Option<Utf8PathBuf> {
    jp_workspace::user_data_dir()
        .ok()
        .map(|d| d.join(PLUGIN_DIR))
}

/// Load the cached registry from disk.
///
/// Returns `None` if no cache exists or it fails to parse.
pub(crate) fn load_cached() -> Option<Registry> {
    let path = cache_path()?;
    let content = std::fs::read_to_string(path.as_std_path()).ok()?;
    match serde_json::from_str(&content) {
        Ok(registry) => Some(registry),
        Err(e) => {
            warn!("Corrupt registry cache: {e}");
            None
        }
    }
}

/// Replace the cached registry with `body`, whole, so a failed write leaves the
/// old copy.
fn write_cache(path: &Utf8Path, body: &str) -> std::io::Result<()> {
    let dir = path.parent().unwrap_or_else(|| Utf8Path::new("."));
    std::fs::create_dir_all(dir)?;

    let mut tmp = NamedUtf8TempFile::new_in(dir)?;
    tmp.write_all(body.as_bytes())?;
    tmp.persist(path).map_err(std::io::Error::other)?;

    Ok(())
}

/// Read a registry as served, and cache it at `cache` when given.
///
/// The cache holds the bytes as served rather than what this version read from
/// them, so an entry this version skips is still there for a newer `jp` sharing
/// the data directory.
fn parse_and_cache(body: &str, cache: Option<&Utf8Path>) -> Result<Registry, cmd::Error> {
    let registry = serde_json::from_str(body)
        .map_err(|error| cmd::Error::from(format!("invalid registry JSON: {error}")))?;

    if let Some(path) = cache {
        match write_cache(path, body) {
            Ok(()) => debug!(%path, "Saved registry cache."),
            Err(error) => warn!(%path, %error, "Failed to cache the plugin registry."),
        }
    }

    Ok(registry)
}

/// Fetch the registry from the server, and cache it.
///
/// A failed fetch leaves the cache as it was.
pub(crate) async fn fetch(client: &reqwest::Client) -> Result<Registry, cmd::Error> {
    debug!(url = REGISTRY_URL, "Fetching plugin registry.");

    let body = client
        .get(REGISTRY_URL)
        .timeout(REGISTRY_TIMEOUT)
        .send()
        .await
        .map_err(|error| cmd::Error::from(format!("failed to fetch registry: {error}")))?
        .error_for_status()
        .map_err(|error| cmd::Error::from(format!("registry server error: {error}")))?
        .text()
        .await
        .map_err(|error| cmd::Error::from(format!("failed to read registry: {error}")))?;

    parse_and_cache(&body, cache_path().as_deref())
}

/// Fetch the registry, falling back to the cached copy if the fetch fails.
///
/// The error is the fetch's, when there is no cached copy either.
pub(crate) async fn fetch_or_load(client: &reqwest::Client) -> Result<Fetched, cmd::Error> {
    match fetch(client).await {
        Ok(registry) => Ok(Fetched::Fresh(registry)),
        Err(error) => {
            warn!(%error, "Failed to fetch the plugin registry; using the cached copy.");
            load_cached().map(Fetched::Cached).ok_or(error)
        }
    }
}

/// A registry, and whether it came from the server just now.
#[derive(Debug)]
pub(crate) enum Fetched {
    /// Fetched just now.
    Fresh(Registry),

    /// The cached copy, because the fetch failed.
    Cached(Registry),
}

impl Fetched {
    /// The registry, wherever it came from.
    pub(crate) fn into_registry(self) -> Registry {
        match self {
            Self::Fresh(registry) | Self::Cached(registry) => registry,
        }
    }
}

/// Download a binary and verify its SHA-256 checksum.
pub(crate) async fn download_and_verify(
    client: &reqwest::Client,
    binary: &RegistryBinary,
) -> Result<Vec<u8>, cmd::Error> {
    debug!(url = %binary.url, "Downloading plugin binary.");
    let resp = client
        .get(&binary.url)
        .send()
        .await
        .map_err(|e| cmd::Error::from(format!("download failed: {e}")))?
        .error_for_status()
        .map_err(|e| cmd::Error::from(format!("download server error: {e}")))?;

    let bytes = resp
        .bytes()
        .await
        .map_err(|e| cmd::Error::from(format!("failed to read download response: {e}")))?;

    let actual = sha256_hex(&bytes);
    if actual != binary.sha256 {
        return Err(cmd::Error::from(format!(
            "checksum mismatch: expected {}, got {actual}",
            binary.sha256
        )));
    }

    debug!("Checksum verified.");
    Ok(bytes.to_vec())
}

/// Install a plugin binary to the user-local bin directory.
///
/// The binary is written beside its final path and renamed over it, so an
/// update never leaves a half-written plugin, and replacing one that is running
/// cannot fail on a busy file.
///
/// Returns the path to the installed binary.
pub(crate) fn install_binary(name: &str, data: &[u8]) -> Result<Utf8PathBuf, cmd::Error> {
    let dir = bin_dir().ok_or("cannot determine user data directory for plugins")?;
    std::fs::create_dir_all(dir.as_std_path())
        .map_err(|e| cmd::Error::from(format!("failed to create plugin bin directory: {e}")))?;

    let path = dir.join(plugin_binary_name(name));

    let mut tmp = NamedUtf8TempFile::new_in(&dir)
        .map_err(|e| cmd::Error::from(format!("failed to create plugin binary: {e}")))?;
    tmp.write_all(data)
        .map_err(|e| cmd::Error::from(format!("failed to write plugin binary: {e}")))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(tmp.path(), std::fs::Permissions::from_mode(0o755))
            .map_err(|e| cmd::Error::from(format!("failed to set executable permission: {e}")))?;
    }

    tmp.persist(&path)
        .map_err(|e| cmd::Error::from(format!("failed to install plugin binary: {e}")))?;

    debug!(path = %path, name, "Installed plugin binary.");
    Ok(path)
}

/// The binary a registry entry publishes for this platform.
pub(crate) fn release(entry: &RegistryPlugin) -> Option<&RegistryBinary> {
    entry.kind.binaries().get(&current_target())
}

/// Find an installed plugin binary by name.
pub(crate) fn find_installed(name: &str) -> Option<Utf8PathBuf> {
    let dir = bin_dir()?;
    let binary_name = plugin_binary_name(name);
    let path = dir.join(&binary_name);

    path.exists().then_some(path)
}

/// Compute the SHA-256 hex digest of a byte slice.
pub(crate) fn sha256_hex(data: &[u8]) -> String {
    use std::fmt::Write as _;
    let hash = Sha256::digest(data);
    hash.iter().fold(String::with_capacity(64), |mut out, b| {
        let _ = write!(out, "{b:02x}");
        out
    })
}

/// Compute the SHA-256 hex digest of a file.
pub(crate) fn sha256_file(path: &Utf8Path) -> Result<String, cmd::Error> {
    let data =
        std::fs::read(path).map_err(|e| cmd::Error::from(format!("failed to read {path}: {e}")))?;
    Ok(sha256_hex(&data))
}

/// Compute the SHA-1 hex digest of a file.
pub(crate) fn sha1_file(path: &Utf8Path) -> Result<String, cmd::Error> {
    let data =
        std::fs::read(path).map_err(|e| cmd::Error::from(format!("failed to read {path}: {e}")))?;
    Ok(format!("{:x}", Sha1::digest(&data)))
}

/// Construct the target triple for the current platform.
///
/// Maps Rust's `std::env::consts` to the target triples used in the registry
/// (e.g. `aarch64-apple-darwin`, `x86_64-unknown-linux-gnu`).
pub(crate) fn current_target() -> String {
    let arch = std::env::consts::ARCH;
    let os_part = match std::env::consts::OS {
        "macos" => "apple-darwin",
        "linux" => "unknown-linux-gnu",
        "windows" => "pc-windows-msvc",
        other => other,
    };
    format!("{arch}-{os_part}")
}

/// Construct the binary filename for a plugin.
fn plugin_binary_name(name: &str) -> String {
    if cfg!(windows) {
        format!("jp-{name}.exe")
    } else {
        format!("jp-{name}")
    }
}

#[cfg(test)]
#[path = "registry_tests.rs"]
mod tests;
