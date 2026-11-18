//! Request origin and `Host` header validation for local HTTP server plugins.
//!
//! A local HTTP server plugin has no authentication, and binding to loopback
//! does not prevent cross-site requests from the user's own browser: a
//! cross-site form submission requires no CORS preflight, and the same-origin
//! policy blocks reading the response rather than sending the request.
//! [`same_origin`] rejects cross-origin browser requests using `Sec-Fetch-Site`
//! and `Origin`.
//!
//! To defend against DNS rebinding (where an attacker-controlled domain
//! resolves to `127.0.0.1`), a loopback server also validates the `Host` header
//! with [`answers`], accepting only IP literals, `localhost` / `*.localhost`,
//! and explicitly configured [`allowed_hosts`].
//!
//! [`allowed_hosts`]: host_allowed

use std::net::IpAddr;

/// Return whether a request originated from the server's own origin, given its
/// `Sec-Fetch-Site`, `Origin`, and `Host` headers.
///
/// If `Sec-Fetch-Site` is present, accepts `"same-origin"` and `"none"` (direct
/// navigation, such as typing the URL into the address bar).
/// Otherwise, if `Origin` is present, requires its authority (`host[:port]`) to
/// match `Host`.
///
/// Requests with neither header are allowed so non-browser clients (`curl`,
/// scripts, CLI tools) continue to work; browsers always attach at least one of
/// these headers on cross-origin state-changing requests.
#[must_use]
pub fn same_origin(sec_fetch_site: Option<&str>, origin: Option<&str>, host: Option<&str>) -> bool {
    if let Some(site) = sec_fetch_site {
        return site == "same-origin" || site == "none";
    }

    let Some(origin) = origin else {
        return true;
    };

    // Compare the `Origin` authority (`scheme://authority`) against `Host`.
    // Reject if `Host` is missing.
    host.is_some_and(|host| {
        origin
            .split_once("://")
            .is_some_and(|(_, authority)| authority == host)
    })
}

/// Return whether a server bound to `listening` accepts a request with the
/// given `Host` header.
///
/// On loopback addresses, delegates to [`host_allowed`] with `allowed`.
/// On non-loopback addresses, accepts any `Host` because remote clients may
/// reach the machine via network hostnames unknown to the server.
#[must_use]
pub fn answers(listening: IpAddr, host: Option<&str>, allowed: &[String]) -> bool {
    !listening.is_loopback() || host_allowed(host, allowed)
}

/// Format the HTTP 403 refusal message for a rejected `Host` header on
/// `plugin`, naming `plugins.command.{plugin}.options.allowed_hosts`.
#[must_use]
pub fn unknown_host(host: &str, plugin: &str) -> String {
    let name = host_name(host).to_ascii_lowercase();

    format!(
        "This server doesn't answer to `{name}`. To reach it by that name, add `{name}` to \
         `plugins.command.{plugin}.options.allowed_hosts`."
    )
}

/// Return whether `host` is an allowed `Host` header value for a loopback
/// server.
///
/// Accepts IP literals, `localhost`, subdomains of `.localhost`, and entries in
/// `allowed` (normalized by [`parse_host`], where a leading `.` matches both
/// the bare domain and all subdomains).
/// Requests without a `Host` header are accepted.
#[must_use]
pub fn host_allowed(host: Option<&str>, allowed: &[String]) -> bool {
    let Some(host) = host else {
        return true;
    };
    let name = host_name(host);
    if name.parse::<IpAddr>().is_ok() {
        return true;
    }

    let name = name.trim_end_matches('.').to_ascii_lowercase();
    name == "localhost"
        || name.ends_with(".localhost")
        || allowed.iter().any(|entry| match entry.strip_prefix('.') {
            Some(domain) => name == domain || name.ends_with(entry.as_str()),
            None => name == *entry,
        })
}

/// Strip the port and IPv6 brackets from a `Host` header value.
fn host_name(host: &str) -> &str {
    let host = host.trim();
    if let Some(rest) = host.strip_prefix('[') {
        return rest.split_once(']').map_or(rest, |(address, _)| address);
    }

    host.rsplit_once(':').map_or(host, |(name, _)| name)
}

/// Parse and normalize an allowed hostname entry (e.g. `"my-machine.local"` or
/// `".example.com"` for a domain and its subdomains), lowercased and without a
/// trailing dot.
///
/// # Errors
///
/// Returns a human-readable reason if `text` is empty or contains characters,
/// schemes, or ports not valid in a hostname.
pub fn parse_host(text: &str) -> Result<String, String> {
    let name = text.trim().trim_end_matches('.').to_ascii_lowercase();
    let bare = name.strip_prefix('.').unwrap_or(&name);
    let valid = !bare.is_empty()
        && bare.split('.').all(|label| {
            !label.is_empty() && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        });

    if !valid {
        return Err(format!(
            "`{text}` is not a host name; write the name alone, without a scheme or a port, such \
             as `my-machine.local`"
        ));
    }

    Ok(name)
}

#[cfg(test)]
#[path = "origin_tests.rs"]
mod tests;
