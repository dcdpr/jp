//! Where links into the repository point on the web.
//!
//! The site links source files to the repository's web address, and a wiki page
//! pins its citations to commits under the same address, so both read it the
//! same way: from a configured `https://` address, or from git's `origin`
//! remote through [`web_address`].

/// Whether `address` is somewhere a browser can follow a link to.
#[must_use]
pub fn is_web_address(address: &str) -> bool {
    let rest = address
        .strip_prefix("https://")
        .or_else(|| address.strip_prefix("http://"));

    rest.is_some_and(|rest| {
        !rest.trim_matches('/').is_empty() && !rest.contains(char::is_whitespace)
    })
}

/// The web address of a git remote, for links to the files in it: no
/// credentials, and no `.git` suffix.
///
/// Reads `https://` and `http://` remotes, `ssh://` and `git://` ones, and the
/// `git@host:owner/repo` form, the last three as `https://` addresses.
/// `None` for a remote no address can be derived from, such as a path on disk.
#[must_use]
pub fn web_address(remote: &str) -> Option<String> {
    let remote = remote.trim();

    // A port on an `ssh://` or `git://` remote belongs to that protocol, and
    // has to go; one on a web remote is part of the web address.
    let (scheme, rest, web) = if let Some(rest) = remote.strip_prefix("https://") {
        ("https", rest, true)
    } else if let Some(rest) = remote.strip_prefix("http://") {
        ("http", rest, true)
    } else if let Some(rest) = remote
        .strip_prefix("ssh://")
        .or_else(|| remote.strip_prefix("git://"))
    {
        ("https", rest, false)
    } else if remote.contains("://") {
        // `file://` and anything else with no web side.
        return None;
    } else if let Some((host, path)) = remote.split_once(':') {
        // `[user@]host:owner/repo`, which git reads as ssh when no slash comes
        // before the colon. A single letter there is a Windows drive.
        if host.contains('/') || host.len() < 2 || path.contains('\\') {
            return None;
        }
        return address("https", host, path, false);
    } else {
        return None;
    };

    let (host, path) = rest.split_once('/')?;
    address(scheme, host, path, web)
}

/// `scheme://host/path`, with the user taken off `host`, and `.git` and slashes
/// off `path`.
///
/// `keep_port` keeps a port on `host`; without it, a port is dropped.
fn address(scheme: &str, host: &str, path: &str, keep_port: bool) -> Option<String> {
    let host = host.rsplit_once('@').map_or(host, |(_, host)| host);
    let host = match host.rsplit_once(':') {
        Some((name, port)) if !keep_port && port.chars().all(|c| c.is_ascii_digit()) => name,
        _ => host,
    };

    let path = path.trim_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    if host.is_empty() || path.is_empty() || host.contains('/') {
        return None;
    }

    Some(format!("{scheme}://{host}/{path}"))
}
