//! Listener configuration, binding, and startup announcement for local HTTP
//! server plugins.
//!
//! Server plugins accept `--bind`, `--port`, and `--allowed-host` CLI flags,
//! falling back to `bind`, `port`, and `allowed_hosts` in
//! `plugins.command.<plugin>.options`, and finally to [`LOOPBACK`] on the
//! plugin's default port ([`Listen::address`], [`Listen::allowed_hosts`]).
//! [`listen`] binds the TCP socket before [`announce`] prints the server URL
//! (and an unauthenticated-access warning when bound to a non-loopback
//! address).

use std::{
    io::Write,
    net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener},
};

use serde_json::{Map, Value};

use crate::{origin::parse_host, stdio};

/// Default bind address (`127.0.0.1`).
pub const LOOPBACK: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);

/// Listener settings from CLI flags or plugin options.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Listen {
    pub bind: Option<IpAddr>,
    pub port: Option<u16>,

    /// Additional hostnames accepted by [`origin::answers`], normalized by
    /// [`parse_host`].
    ///
    /// [`origin::answers`]: crate::origin::answers
    pub allowed_hosts: Option<Vec<String>>,
}

impl Listen {
    /// Parse `bind`, `port`, and `allowed_hosts` from
    /// `plugins.command.{plugin}.options`.
    ///
    /// # Errors
    ///
    /// Returns an error if `bind` is not a valid bind address ([`parse_bind`]),
    /// `port` is not an integer in `0..=65535`, or `allowed_hosts` is not an
    /// array of valid hostnames ([`parse_host`]).
    pub fn from_options(options: &Map<String, Value>, plugin: &str) -> Result<Self, String> {
        let key = |name: &str| format!("`plugins.command.{plugin}.options.{name}`");

        let bind = match options.get("bind") {
            None | Some(Value::Null) => None,
            Some(Value::String(text)) => {
                Some(parse_bind(text).map_err(|why| format!("{}: {why}.", key("bind")))?)
            }
            Some(other) => {
                return Err(format!(
                    "{} must be an IP address or `localhost`, not {other}.",
                    key("bind")
                ));
            }
        };
        let port = match options.get("port") {
            None | Some(Value::Null) => None,
            Some(value) => Some(
                value
                    .as_u64()
                    .and_then(|port| u16::try_from(port).ok())
                    .ok_or_else(|| {
                        format!(
                            "{} must be a port, from 0 to 65535, not {value}.",
                            key("port")
                        )
                    })?,
            ),
        };

        Ok(Self {
            bind,
            port,
            allowed_hosts: allowed_hosts(options.get("allowed_hosts"), &key("allowed_hosts"))?,
        })
    }

    /// Resolve the socket address to bind, preferring `flags`, then `self`,
    /// then [`LOOPBACK`] on `default_port`.
    #[must_use]
    pub fn address(&self, flags: &Self, default_port: u16) -> SocketAddr {
        SocketAddr::new(
            flags.bind.or(self.bind).unwrap_or(LOOPBACK),
            flags.port.or(self.port).unwrap_or(default_port),
        )
    }

    /// Return the configured allowed hostnames, preferring `flags` over `self`.
    #[must_use]
    pub fn allowed_hosts<'a>(&'a self, flags: &'a Self) -> &'a [String] {
        flags
            .allowed_hosts
            .as_deref()
            .or(self.allowed_hosts.as_deref())
            .unwrap_or_default()
    }
}

/// Parse the `allowed_hosts` option array at config path `key`.
fn allowed_hosts(value: Option<&Value>, key: &str) -> Result<Option<Vec<String>>, String> {
    let entries = match value {
        None | Some(Value::Null) => return Ok(None),
        Some(Value::Array(entries)) => entries,
        Some(other) => {
            return Err(format!(
                "{key} must be a list of host names, such as `[\"my-machine.local\"]`, not \
                 {other}."
            ));
        }
    };

    entries
        .iter()
        .map(|entry| match entry {
            Value::String(name) => parse_host(name).map_err(|why| format!("{key}: {why}.")),
            other => Err(format!("{key}: {other} is not a host name.")),
        })
        .collect::<Result<_, _>>()
        .map(Some)
}

/// Parse a bind address from an IP literal (with optional IPv6 brackets) or
/// `"localhost"` (mapped to [`LOOPBACK`]).
///
/// # Errors
///
/// Returns an error message if `text` is not an IP address or `"localhost"`.
pub fn parse_bind(text: &str) -> Result<IpAddr, String> {
    let trimmed = text.trim();
    if trimmed.eq_ignore_ascii_case("localhost") {
        return Ok(LOOPBACK);
    }

    let bare = trimmed
        .strip_prefix('[')
        .and_then(|rest| rest.strip_suffix(']'))
        .unwrap_or(trimmed);
    bare.parse()
        .map_err(|_| format!("`{text}` is not an IP address, such as `127.0.0.1`, or `localhost`"))
}

/// Bind a non-blocking [`TcpListener`] at `address` for use with an async
/// runtime.
///
/// # Errors
///
/// Returns an error if binding or setting non-blocking mode fails.
pub fn listen(address: SocketAddr) -> Result<TcpListener, String> {
    let failed = |error| format!("Failed to listen on {address}: {error}");
    let listener = TcpListener::bind(address).map_err(failed)?;
    listener.set_nonblocking(true).map_err(failed)?;

    Ok(listener)
}

/// Print the `Serving <what> at <url>` message to `stdout`, followed by a
/// security warning when `address` is non-loopback.
///
/// `reach` describes what an unauthenticated client on the network can do (e.g.
/// `"read and modify conversations"`).
///
/// # Errors
///
/// Returns an error if writing to `stdout` fails.
pub fn announce(
    stdout: &mut impl Write,
    what: &str,
    address: SocketAddr,
    reach: &str,
) -> Result<(), String> {
    let ip = address.ip();
    stdio::print_to(
        stdout,
        &format!("Serving {what} at {}\n", url(address)),
        "content",
    )?;
    if ip.is_loopback() {
        return Ok(());
    }

    let listening = if ip.is_unspecified() {
        "every address of this machine".to_owned()
    } else {
        ip.to_string()
    };
    // Emit on `content` rather than `chrome` so `--quiet` does not suppress
    // the security warning.
    stdio::print_to(
        stdout,
        &format!(
            "Warning: listening on {listening}, which other machines can reach, without \
             authentication: anyone who reaches it can {reach}.\n"
        ),
        "content",
    )
}

/// Format the local `http://` URL for `address`, substituting `localhost` when
/// bound to an unspecified address (`0.0.0.0` or `::`).
#[must_use]
pub fn url(address: SocketAddr) -> String {
    if address.ip().is_unspecified() {
        return format!("http://localhost:{}", address.port());
    }

    format!("http://{address}")
}

#[cfg(test)]
#[path = "server_tests.rs"]
mod tests;
