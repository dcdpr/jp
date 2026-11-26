use std::net::{Ipv6Addr, TcpListener as StdListener};

use pretty_assertions::assert_eq;
use serde_json::json;

use super::*;

fn options(value: Value) -> Map<String, Value> {
    match value {
        Value::Object(map) => map,
        other => panic!("not an object: {other}"),
    }
}

#[test]
fn an_ip_address_or_localhost_is_an_address_to_listen_on() {
    assert_eq!(parse_bind("127.0.0.1"), Ok(LOOPBACK));
    assert_eq!(parse_bind(" localhost "), Ok(LOOPBACK));
    assert_eq!(parse_bind("LocalHost"), Ok(LOOPBACK));
    assert_eq!(parse_bind("0.0.0.0"), Ok(IpAddr::V4(Ipv4Addr::UNSPECIFIED)));
    assert_eq!(parse_bind("::"), Ok(IpAddr::V6(Ipv6Addr::UNSPECIFIED)));
    assert_eq!(parse_bind("[::1]"), Ok(IpAddr::V6(Ipv6Addr::LOCALHOST)));
    assert_eq!(
        parse_bind("example.test"),
        Err("`example.test` is not an IP address, such as `127.0.0.1`, or `localhost`".to_owned())
    );
}

#[test]
fn the_options_name_an_address_and_a_port() {
    let read = Listen::from_options(&options(json!({ "bind": "0.0.0.0", "port": 8080 })), "x");

    assert_eq!(
        read,
        Ok(Listen {
            bind: Some(IpAddr::V4(Ipv4Addr::UNSPECIFIED)),
            port: Some(8080),
            allowed_hosts: None,
        })
    );
    assert_eq!(
        Listen::from_options(&Map::new(), "x"),
        Ok(Listen::default())
    );
}

/// A value that can't be used is refused, naming the key, rather than replaced
/// by the default without a word.
#[test]
fn an_option_that_isnt_an_address_or_a_port_is_refused() {
    let refused = |value: Value| Listen::from_options(&options(value), "serve-web").unwrap_err();

    assert_eq!(
        refused(json!({ "bind": "my-machine" })),
        "`plugins.command.serve-web.options.bind`: `my-machine` is not an IP address, such as \
         `127.0.0.1`, or `localhost`."
    );
    assert_eq!(
        refused(json!({ "bind": 127 })),
        "`plugins.command.serve-web.options.bind` must be an IP address or `localhost`, not 127."
    );
    assert_eq!(
        refused(json!({ "port": 70000 })),
        "`plugins.command.serve-web.options.port` must be a port, from 0 to 65535, not 70000."
    );
    assert_eq!(
        refused(json!({ "port": "8080" })),
        "`plugins.command.serve-web.options.port` must be a port, from 0 to 65535, not \"8080\"."
    );
    assert_eq!(
        refused(json!({ "allowed_hosts": "my-machine.local" })),
        "`plugins.command.serve-web.options.allowed_hosts` must be a list of host names, such as \
         `[\"my-machine.local\"]`, not \"my-machine.local\"."
    );
    assert_eq!(
        refused(json!({ "allowed_hosts": ["box:3000"] })),
        "`plugins.command.serve-web.options.allowed_hosts`: `box:3000` is not a host name; write \
         the name alone, without a scheme or a port, such as `my-machine.local`."
    );
}

/// A name the flags give replaces the options' list rather than adding to it,
/// as every flag replaces its option.
#[test]
fn the_allowed_hosts_are_the_flags_then_the_options() {
    let options = Listen::from_options(
        &options(json!({ "allowed_hosts": ["My-Machine.local", ".ts.net"] })),
        "x",
    )
    .unwrap();
    let flags = Listen {
        allowed_hosts: Some(vec!["other.local".to_owned()]),
        ..Listen::default()
    };

    assert_eq!(options.allowed_hosts(&Listen::default()), [
        "my-machine.local",
        ".ts.net"
    ]);
    assert_eq!(options.allowed_hosts(&flags), ["other.local"]);
    assert_eq!(Listen::default().allowed_hosts(&Listen::default()), []
        as [String; 0]);
}

/// A flag wins over the option, and the option over the default.
#[test]
fn the_address_is_the_flags_then_the_options_then_loopback() {
    let unset = Listen::default();
    let options = Listen {
        bind: Some(IpAddr::V4(Ipv4Addr::UNSPECIFIED)),
        port: Some(8080),
        allowed_hosts: None,
    };
    let flags = Listen {
        bind: Some(IpAddr::V6(Ipv6Addr::LOCALHOST)),
        ..Listen::default()
    };

    assert_eq!(unset.address(&unset, 3001).to_string(), "127.0.0.1:3001");
    assert_eq!(options.address(&unset, 3001).to_string(), "0.0.0.0:8080");
    assert_eq!(options.address(&flags, 3001).to_string(), "[::1]:8080");
}

#[test]
fn listening_on_a_held_port_names_the_address() {
    let held = StdListener::bind("127.0.0.1:0").unwrap();
    let address = held.local_addr().unwrap();

    let error = listen(address).unwrap_err();

    assert!(
        error.starts_with(&format!("Failed to listen on {address}: ")),
        "{error}"
    );
}

/// What `announce` prints for a server on `address`.
fn announced(address: &str) -> Vec<String> {
    let mut stdout = vec![];
    announce(
        &mut stdout,
        "reviews",
        address.parse().unwrap(),
        "read every review",
    )
    .unwrap();

    String::from_utf8(stdout)
        .unwrap()
        .lines()
        .map(|line| match serde_json::from_str(line).unwrap() {
            crate::PluginToHost::Print(print) => print.text,
            other => panic!("not a print: {other:?}"),
        })
        .collect()
}

#[test]
fn a_loopback_server_is_announced_without_a_warning() {
    assert_eq!(announced("127.0.0.1:3001"), [
        "Serving reviews at http://127.0.0.1:3001\n"
    ]);
    assert_eq!(announced("[::1]:3001"), [
        "Serving reviews at http://[::1]:3001\n"
    ]);
}

/// `0.0.0.0` is opened at `localhost`, and other machines reach it too.
#[test]
fn a_server_on_every_address_is_opened_at_localhost_with_a_warning() {
    assert_eq!(announced("0.0.0.0:3001"), [
        "Serving reviews at http://localhost:3001\n",
        "Warning: listening on every address of this machine, which other machines can reach, \
         without authentication: anyone who reaches it can read every review.\n",
    ]);
}

#[test]
fn a_server_on_one_other_address_names_it_in_the_warning() {
    assert_eq!(announced("192.168.1.5:3001"), [
        "Serving reviews at http://192.168.1.5:3001\n",
        "Warning: listening on 192.168.1.5, which other machines can reach, without \
         authentication: anyone who reaches it can read every review.\n",
    ]);
}
