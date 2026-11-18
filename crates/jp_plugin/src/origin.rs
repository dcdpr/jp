//! Whether a request to a plugin's local web server came from one of the
//! server's own pages.
//!
//! A plugin serving pages to the user's browser has no authentication, and
//! binding to loopback doesn't help: a request any page sends comes from the
//! user's own browser, which is already inside.
//! A form post needs no preflight, so any page the browser visits can send one,
//! and the same-origin policy stops that page reading the answer, not sending
//! the request.
//! [`same_origin`] tells the two apart by the headers the browser adds.
//!
//! A page on a domain whose address its owner re-points at this machine (DNS
//! rebinding) reaches the server as its own origin, and passes that check.
//! The browser still names that domain in `Host`, so a server listening on
//! loopback answers only the names this machine is reached by, and the ones the
//! user allows ([`answers`]).

use std::net::IpAddr;

/// Whether a request came from one of the server's own pages, given its
/// `Sec-Fetch-Site`, `Origin`, and `Host` headers.
///
/// `Sec-Fetch-Site` is the browser's own answer and is taken where it is given:
/// `same-origin`, or `none` for a request no page made, such as one typed into
/// the address bar.
/// `Origin` against `Host` is the fallback for a browser too old to send it: a
/// cross-site form post carries the submitting page's origin, which is not the
/// server's.
///
/// A request carrying neither is allowed.
/// `curl`, a script, or another tool sends no such header, and no browser can
/// be made to omit both, so refusing it would lock out every caller that isn't
/// a browser to stop nothing.
#[must_use]
pub fn same_origin(sec_fetch_site: Option<&str>, origin: Option<&str>, host: Option<&str>) -> bool {
    if let Some(site) = sec_fetch_site {
        return site == "same-origin" || site == "none";
    }

    let Some(origin) = origin else {
        return true;
    };

    // `Host` is the authority the browser was told to connect to, which is the
    // authority half of the origin the server's own pages carry. Without it
    // there is nothing to compare against, and guessing is not worth it.
    host.is_some_and(|host| {
        origin
            .split_once("://")
            .is_some_and(|(_, authority)| authority == host)
    })
}

/// Whether a server listening on `listening` answers a request whose `Host`
/// header is `host`.
///
/// On loopback it answers what [`host_allowed`] does, the names in `allowed`
/// included.
/// On any other address it answers every name, since other machines reach it by
/// names this one can't know, such as the one their network's DNS gives it.
#[must_use]
pub fn answers(listening: IpAddr, host: Option<&str>, allowed: &[String]) -> bool {
    !listening.is_loopback() || host_allowed(host, allowed)
}

/// The refusal for a request whose `Host` header, `host`, a server of the
/// plugin `plugin` doesn't answer to, naming the option that allows it.
#[must_use]
pub fn unknown_host(host: &str, plugin: &str) -> String {
    let name = host_name(host).to_ascii_lowercase();

    format!(
        "This server doesn't answer to `{name}`. To reach it by that name, add `{name}` to \
         `plugins.command.{plugin}.options.allowed_hosts`."
    )
}

/// Whether the server answers a request whose `Host` header is `host`.
///
/// It answers an IP address, `localhost`, a name under `.localhost`, and a name
/// `allowed` lists, each as [`parse_host`] reads it: an entry starting with `.`
/// stands for the name after it and every name under it.
/// A request with no `Host` is answered: no browser sends one.
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

/// The name in a `Host` header, without its port or an IPv6 address's brackets.
fn host_name(host: &str) -> &str {
    let host = host.trim();
    if let Some(rest) = host.strip_prefix('[') {
        return rest.split_once(']').map_or(rest, |(address, _)| address);
    }

    host.rsplit_once(':').map_or(host, |(name, _)| name)
}

/// Read a host name a server is allowed to answer to, such as
/// `my-machine.local`, or `.example.com` for that name and every name under it;
/// lowercase, without a trailing dot.
///
/// # Errors
///
/// What is wrong with `text`, for the caller to name where it came from.
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
