use std::net::{Ipv4Addr, Ipv6Addr};

use pretty_assertions::assert_eq;

use super::*;

#[test]
fn the_browser_saying_same_origin_is_believed() {
    assert!(same_origin(
        Some("same-origin"),
        Some("http://127.0.0.1:3000"),
        Some("127.0.0.1:3000")
    ));
    assert!(!same_origin(
        Some("cross-site"),
        Some("http://127.0.0.1:3000"),
        Some("127.0.0.1:3000")
    ));
    assert!(!same_origin(Some("same-site"), None, None));
}

/// There is no page behind it to be acting on anyone's behalf.
#[test]
fn a_request_no_page_made_is_allowed() {
    assert!(same_origin(Some("none"), None, None));
}

/// A browser too old to send `Sec-Fetch-Site` still sends `Origin`, and the
/// authority it names is either this server or somebody else.
#[test]
fn without_sec_fetch_site_the_origin_has_to_be_the_host() {
    assert!(same_origin(
        None,
        Some("http://127.0.0.1:3000"),
        Some("127.0.0.1:3000")
    ));
    assert!(!same_origin(
        None,
        Some("https://example.test"),
        Some("127.0.0.1:3000")
    ));
    assert!(!same_origin(None, Some("null"), Some("127.0.0.1:3000")));
    assert!(!same_origin(None, Some("http://127.0.0.1:3000"), None));
}

/// `curl` and scripts send neither header.
#[test]
fn a_request_without_either_header_is_allowed() {
    assert!(same_origin(None, None, Some("127.0.0.1:3000")));
    assert!(same_origin(None, None, None));
}

/// A name the browser resolves to this machine on its own, or an address, can't
/// be a domain someone else re-pointed here.
#[test]
fn an_address_or_localhost_is_answered() {
    for host in [
        "127.0.0.1:3001",
        "192.168.1.5:3001",
        "[::1]:3001",
        "[::1]",
        "localhost:3001",
        "LocalHost",
        "localhost.",
        "review.localhost:3001",
    ] {
        assert!(host_allowed(Some(host), &[]), "{host}");
    }
}

/// The name a page re-pointed at this machine still carries.
#[test]
fn another_name_is_answered_only_when_allowed() {
    let allowed = ["my-machine.local".to_owned(), ".ts.net".to_owned()];

    assert!(!host_allowed(Some("evil.example:3001"), &[]));
    assert!(!host_allowed(Some("evil.example:3001"), &allowed));
    assert!(!host_allowed(Some("localhost.evil.example"), &allowed));
    assert!(host_allowed(Some("My-Machine.local:3001"), &allowed));
    assert!(host_allowed(Some("ts.net"), &allowed));
    assert!(host_allowed(Some("box.tail1234.ts.net:3001"), &allowed));
    assert!(!host_allowed(Some("notts.net"), &allowed));
}

/// No browser leaves `Host` out.
#[test]
fn a_request_without_a_host_is_answered() {
    assert!(host_allowed(None, &[]));
}

/// Listening beyond loopback, the server is reached by names other machines
/// know it by, which it can't list.
#[test]
fn only_a_server_on_loopback_checks_the_name() {
    let loopback = IpAddr::V4(Ipv4Addr::LOCALHOST);
    let tailnet = [".ts.net".to_owned()];

    assert!(!answers(loopback, Some("evil.example:3001"), &[]));
    assert!(answers(loopback, Some("localhost:3001"), &[]));
    assert!(answers(
        IpAddr::V6(Ipv6Addr::LOCALHOST),
        Some("box.tail1234.ts.net"),
        &tailnet
    ));
    assert!(answers(
        IpAddr::V4(Ipv4Addr::UNSPECIFIED),
        Some("evil.example:3001"),
        &[]
    ));
}

#[test]
fn the_refusal_names_the_option_that_allows_the_name() {
    assert_eq!(
        unknown_host("Evil.Example:3001", "review"),
        "This server doesn't answer to `evil.example`. To reach it by that name, add \
         `evil.example` to `plugins.command.review.options.allowed_hosts`."
    );
}

#[test]
fn a_host_name_is_read_without_its_case_or_trailing_dot() {
    assert_eq!(
        parse_host(" My-Machine.Local. "),
        Ok("my-machine.local".to_owned())
    );
    assert_eq!(parse_host(".ts.net"), Ok(".ts.net".to_owned()));

    for refused in ["", ".", "box:3001", "http://box", "a..b", "my box"] {
        assert_eq!(
            parse_host(refused),
            Err(format!(
                "`{refused}` is not a host name; write the name alone, without a scheme or a \
                 port, such as `my-machine.local`"
            )),
            "{refused}"
        );
    }
}
