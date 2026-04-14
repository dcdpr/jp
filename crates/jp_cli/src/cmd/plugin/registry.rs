//! Plugin registry operations.
//!
//! Handles fetching, caching, and querying the plugin registry,
//! as well as downloading and installing plugin binaries.

use camino::{Utf8Path, Utf8PathBuf};
use jp_plugin::registry::{ApprovedPlugin, PluginApprovals, Registry, RegistryBinary};
use sha2::{Digest, Sha256};
use tracing::{debug, warn};

use crate::cmd;

/// The URL of the official JP plugin registry.
const REGISTRY_URL: &str = "https://jp.computer/plugins.json";

/// Filename for the cached registry.
const REGISTRY_CACHE_FILE: &str = "registry.json";

/// Directory path for installed command plugin binaries.
const PLUGIN_DIR: &str = "plugins/command";

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

/// Save the registry to the local cache file.
pub(crate) fn save_cache(registry: &Registry) -> Result<(), cmd::Error> {
    let path =
        cache_path().ok_or_else(|| cmd::Error::from("cannot determine user data directory"))?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent.as_std_path())
            .map_err(|e| cmd::Error::from(format!("failed to create cache directory: {e}")))?;
    }
    let json = serde_json::to_string_pretty(registry)
        .map_err(|e| cmd::Error::from(format!("failed to serialize registry: {e}")))?;
    std::fs::write(path.as_std_path(), json)
        .map_err(|e| cmd::Error::from(format!("failed to write registry cache: {e}")))?;
    debug!(path = %path, "Saved registry cache.");
    Ok(())
}

/// Fetch the registry from the server.
pub(crate) async fn fetch(client: &reqwest::Client) -> Result<Registry, cmd::Error> {
    debug!(url = REGISTRY_URL, "Fetching plugin registry.");

    let resp = client
        .get(REGISTRY_URL)
        .send()
        .await
        .map_err(|error| cmd::Error::from(format!("failed to fetch registry: {error}")))?
        .error_for_status()
        .map_err(|error| cmd::Error::from(format!("registry server error: {error}")))?;

    resp.json()
        .await
        .map_err(|error| cmd::Error::from(format!("invalid registry JSON: {error}")))
}

/// Fetch the registry and update the cache, falling back to the cached copy if
/// the fetch fails.
pub(crate) async fn fetch_or_load(client: &reqwest::Client) -> Result<Registry, cmd::Error> {
    match fetch(client).await {
        Ok(registry) => {
            if let Err(error) = save_cache(&registry) {
                warn!(%error, "Failed to cache registry.");
            }

            Ok(registry)
        }
        Err(error) => {
            warn!(%error, "Failed to fetch registry, trying cache.");
            load_cached().ok_or(error)
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
/// Returns the path to the installed binary.
pub(crate) fn install_binary(name: &str, data: &[u8]) -> Result<Utf8PathBuf, cmd::Error> {
    let dir = bin_dir()
        .ok_or_else(|| cmd::Error::from("cannot determine user data directory for plugins"))?;
    std::fs::create_dir_all(dir.as_std_path())
        .map_err(|e| cmd::Error::from(format!("failed to create plugin bin directory: {e}")))?;

    let binary_name = plugin_binary_name(name);
    let path = dir.join(&binary_name);
    std::fs::write(path.as_std_path(), data)
        .map_err(|e| cmd::Error::from(format!("failed to write plugin binary: {e}")))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(path.as_std_path(), std::fs::Permissions::from_mode(0o755))
            .map_err(|e| cmd::Error::from(format!("failed to set executable permission: {e}")))?;
    }

    debug!(path = %path, name, "Installed plugin binary.");
    Ok(path)
}

/// Find an installed plugin binary by name.
pub(crate) fn find_installed(name: &str) -> Option<Utf8PathBuf> {
    let dir = bin_dir()?;
    let binary_name = plugin_binary_name(name);
    let path = dir.join(&binary_name);

    path.exists().then_some(path)
}

/// List all installed plugins in the bin directory.
///
/// Returns `(name, path)` pairs sorted by name.
pub(crate) fn discover_installed() -> Vec<(String, Utf8PathBuf)> {
    let Some(dir) = bin_dir() else {
        return Vec::new();
    };

    let Ok(entries) = dir.read_dir_utf8() else {
        return Vec::new();
    };

    let mut plugins: Vec<_> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name();
            let subcommand = strip_plugin_prefix(name)?;
            Some((subcommand.to_owned(), entry.into_path()))
        })
        .collect();

    plugins.sort_by(|a, b| a.0.cmp(&b.0));
    plugins
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

/// Strip the `jp-` prefix (and `.exe` suffix on Windows) from a filename,
/// returning the plugin subcommand name.
fn strip_plugin_prefix(filename: &str) -> Option<&str> {
    let name = filename.strip_prefix("jp-")?;

    #[cfg(windows)]
    let name = name.strip_suffix(".exe").unwrap_or(name);

    Some(name)
}

/// Filename for the locally stored plugin approval records.
const APPROVALS_FILE: &str = "plugin-approvals.json";

/// Load permanent plugin approvals from disk.
///
/// Returns `None` if the file doesn't exist or fails to parse.
pub(crate) fn load_approvals() -> Option<PluginApprovals> {
    let path = jp_workspace::user_data_dir().ok()?.join(APPROVALS_FILE);
    let content = std::fs::read_to_string(path.as_std_path()).ok()?;
    match serde_json::from_str(&content) {
        Ok(approvals) => Some(approvals),
        Err(e) => {
            warn!("Corrupt plugin approvals file: {e}");
            None
        }
    }
}

/// Save a permanent approval for a PATH-discovered plugin.
///
/// Records the binary path and its SHA-256 digest at time of approval.
/// On subsequent runs, if both match, the plugin runs without prompting.
pub(crate) fn save_approval(name: &str, binary_path: &Utf8Path) -> Result<(), cmd::Error> {
    let data_dir = jp_workspace::user_data_dir()
        .map_err(|_| cmd::Error::from("cannot determine user data directory"))?;
    let path = data_dir.join(APPROVALS_FILE);

    let mut approvals = load_approvals().unwrap_or_default();
    let sha256 = sha256_file(binary_path)?;

    approvals.approved.insert(name.to_owned(), ApprovedPlugin {
        path: binary_path.to_owned(),
        sha256,
    });

    let json = serde_json::to_string_pretty(&approvals)
        .map_err(|e| cmd::Error::from(format!("failed to serialize approvals: {e}")))?;
    std::fs::write(path.as_std_path(), json)
        .map_err(|e| cmd::Error::from(format!("failed to write approvals: {e}")))?;

    debug!("Saved plugin approval for `{name}`.");
    Ok(())
}

#[cfg(test)]
#[path = "registry_tests.rs"]
mod tests;
