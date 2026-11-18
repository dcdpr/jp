//! Where a plugin's local web server listens, what it prints when it starts,
//! and which names it answers to.
//!
//! A server takes `--bind` and `--port`, falls back to the `bind` and `port`
//! keys of its options, then to loopback on its own port ([`Listen::address`]).
//! Listening on loopback, it answers addresses, `localhost`, and the names
//! `--allowed-host` or the `allowed_hosts` option lists
//! ([`Listen::allowed_hosts`]), checking each request's `Host` against them
//! (`origin::answers`).
//! [`listen`] binds the address before anything is printed, so a port in use is
//! an error rather than a "Serving" line for a server that isn't, and
//! [`announce`] prints where to open the server, with a warning when other
//! machines can reach it.

use std::{
    io::Write,
    net::{IpAddr, Ipv4Addr, SocketAddr, TcpListener},
};

use serde_json::{Map, Value};

use crate::{origin::parse_host, stdio};

/// The address a server listens on when nothing names one: loopback, which only
/// this machine reaches.
pub const LOOPBACK: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);

/// Where a server is told to listen, and the names it answers to, by its flags
/// or by its options; what is unset falls back ([`Listen::address`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Listen {
    pub bind: Option<IpAddr>,
    pub port: Option<u16>,

    /// The names, besides addresses and `localhost`, the server answers to, as
    /// [`parse_host`] reads them; `None` when not given.
    pub allowed_hosts: Option<Vec<String>>,
}

impl Listen {
    /// The `bind`, `port`, and `allowed_hosts` keys of `options`, the options
    /// of the plugin `plugin`.
    ///
    /// # Errors
    ///
    /// When `bind` isn't an address [`parse_bind`] reads, `port` isn't a whole
    /// number from 0 to 65535, or `allowed_hosts` isn't a list of names
    /// [`parse_host`] reads.
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

    /// Where to listen: `flags` where they are set, then these, then loopback
    /// on `default_port`.
    #[must_use]
    pub fn address(&self, flags: &Self, default_port: u16) -> SocketAddr {
        SocketAddr::new(
            flags.bind.or(self.bind).unwrap_or(LOOPBACK),
            flags.port.or(self.port).unwrap_or(default_port),
        )
    }

    /// The names to answer to besides addresses and `localhost`: `flags`' when
    /// they name any, and otherwise these.
    #[must_use]
    pub fn allowed_hosts<'a>(&'a self, flags: &'a Self) -> &'a [String] {
        flags
            .allowed_hosts
            .as_deref()
            .or(self.allowed_hosts.as_deref())
            .unwrap_or_default()
    }
}

/// The `allowed_hosts` option, `value`, which `key` names: a list of names.
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

/// Read an address to listen on: an IP address, such as `127.0.0.1` or
/// `0.0.0.0`, with or without brackets around an IPv6 one, or `localhost`,
/// which is `127.0.0.1`.
///
/// # Errors
///
/// What is wrong with `text`, for the caller to name where it came from.
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

/// Listen on `address`, ready for an async runtime to take the listener over.
///
/// # Errors
///
/// When the address can't be bound, as when another server holds the port.
pub fn listen(address: SocketAddr) -> Result<TcpListener, String> {
    let failed = |error| format!("Failed to listen on {address}: {error}");
    let listener = TcpListener::bind(address).map_err(failed)?;
    listener.set_nonblocking(true).map_err(failed)?;

    Ok(listener)
}

/// Print where to open the server serving `what` on `address`, and warn when
/// other machines can reach it, naming what anyone who does can do: `reach`.
///
/// # Errors
///
/// When `stdout` refuses the message.
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
    // On `content`, not `chrome`: `--quiet` hides chrome, and this is a
    // warning the user has to see.
    stdio::print_to(
        stdout,
        &format!(
            "Warning: listening on {listening}, which other machines can reach, without \
             authentication: anyone who reaches it can {reach}.\n"
        ),
        "content",
    )
}

/// The address to open the server on `address` at from this machine.
///
/// A server listening on every address is opened at `localhost`, since a
/// browser may refuse `0.0.0.0`.
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
