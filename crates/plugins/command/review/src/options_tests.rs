use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

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
fn every_option_is_optional() {
    assert_eq!(Options::read(&Map::new()).unwrap(), Options::default());
}

#[test]
fn the_defaults_name_the_assistant_and_bound_its_text() {
    let defaults = Options::default();

    assert_eq!(defaults.assistant, "assistant");
    assert_eq!(defaults.max_chars, 600);
    assert_eq!(defaults.cfg, [] as [String; 0]);
}

/// Keys a newer `jp-review` reads are left alone rather than refused.
#[test]
fn every_option_is_read_and_unknown_keys_are_ignored() {
    let read = Options::read(&options(json!({
        "dir": ".jp/reviews",
        "cfg": ["skill/review", "assistant.model.id=opus"],
        "base": ["git", "show", "HEAD:./{path}"],
        "assistant": " jp ",
        "max_chars": 300,
        "bind": " 0.0.0.0 ",
        "port": 8080,
        "allowed_hosts": ["My-Machine.local"],
        "something_newer": true
    })))
    .unwrap();

    assert_eq!(read, Options {
        dir: Some(Utf8PathBuf::from(".jp/reviews")),
        cfg: vec![
            "skill/review".to_owned(),
            "assistant.model.id=opus".to_owned()
        ],
        base: Some(vec![
            "git".to_owned(),
            "show".to_owned(),
            "HEAD:./{path}".to_owned()
        ]),
        assistant: "jp".to_owned(),
        max_chars: 300,
        listen: Listen {
            bind: Some(IpAddr::V4(Ipv4Addr::UNSPECIFIED)),
            port: Some(8080),
            allowed_hosts: Some(vec!["my-machine.local".to_owned()]),
        },
    });
}

/// `localhost` is the most common way to name loopback.
#[test]
fn bind_is_an_ip_address_or_localhost_and_port_fits_a_port() {
    let localhost = Options::read(&options(json!({ "bind": "localhost" }))).unwrap();

    assert_eq!(localhost.listen.bind, Some(IpAddr::V4(Ipv4Addr::LOCALHOST)));
    assert_eq!(
        Options::read(&options(json!({ "bind": "my-machine" }))).unwrap_err(),
        "`plugins.command.review.options.bind`: `my-machine` is not an IP address, such as \
         `127.0.0.1`, or `localhost`."
    );
    assert_eq!(
        Options::read(&options(json!({ "port": 70000 }))).unwrap_err(),
        "`plugins.command.review.options.port` must be a port, from 0 to 65535, not 70000."
    );
}

/// A flag given on the command line wins over the option, and the option over
/// the default.
#[test]
fn the_address_to_listen_on_is_the_flags_then_the_options_then_loopback() {
    let unset = Options::default();
    let set = Options {
        listen: Listen {
            bind: Some(IpAddr::V4(Ipv4Addr::UNSPECIFIED)),
            port: Some(8080),
            allowed_hosts: None,
        },
        ..Options::default()
    };
    let flags = |bind, port| Listen {
        bind,
        port,
        allowed_hosts: None,
    };
    let loopback_v6 = Some(IpAddr::V6(Ipv6Addr::LOCALHOST));

    assert_eq!(
        unset.listen_on(&Listen::default()).to_string(),
        "127.0.0.1:3001"
    );
    assert_eq!(
        set.listen_on(&Listen::default()).to_string(),
        "0.0.0.0:8080"
    );
    assert_eq!(
        set.listen_on(&flags(loopback_v6, Some(0))).to_string(),
        "[::1]:0"
    );
    assert_eq!(
        set.listen_on(&flags(None, Some(9000))).to_string(),
        "0.0.0.0:9000"
    );
}

#[test]
fn dir_on_the_command_line_wins_over_the_option() {
    let read = Options::read(&options(json!({
        "dir": ".jp/reviews",
        "cfg": ["skill/review"]
    })))
    .unwrap();
    let overrides = Overrides {
        dir: Some(Utf8PathBuf::from("tmp/reviews")),
        ..Overrides::default()
    };

    let with = read.clone().with(&overrides);

    assert_eq!(with.dir, Some(Utf8PathBuf::from("tmp/reviews")));
    assert_eq!(with.cfg, ["skill/review"]);
    assert_eq!(read.clone().with(&Overrides::default()), read);
}

/// The tools that answer a pass read the turn's options, which for an existing
/// conversation are the ones it was created with, so a pass names the values
/// this process holds, ahead of `cfg`.
#[test]
fn a_pass_forwards_the_options_its_tools_read_before_cfg() {
    let read = Options::read(&options(json!({
        "dir": "tmp/reviews",
        "cfg": ["skill/review"],
        "assistant": "jp \"bot\"",
        "max_chars": 200
    })))
    .unwrap();

    assert_eq!(read.pass_cfg(), [
        r#"plugins.command.review.options.assistant:="jp \"bot\"""#,
        "plugins.command.review.options.max_chars:=200",
        r#"plugins.command.review.options.dir:="tmp/reviews""#,
        "skill/review",
    ]);
}

/// Unset, `dir` is the user-local default, which a turn resolves the same way.
#[test]
fn a_pass_forwards_no_dir_when_none_is_set() {
    assert_eq!(Options::default().pass_cfg(), [
        r#"plugins.command.review.options.assistant:="assistant""#,
        "plugins.command.review.options.max_chars:=600",
    ]);
}

#[test]
fn the_assistant_is_a_name_on_one_line_and_max_chars_allows_some() {
    assert_eq!(
        Options::read(&options(json!({ "assistant": "j\np" }))).unwrap_err(),
        "`plugins.command.review.options.assistant` must be a name on one line."
    );
    assert_eq!(
        Options::read(&options(json!({ "assistant": " " }))).unwrap_err(),
        "`plugins.command.review.options.assistant` must be a name on one line."
    );
    assert_eq!(
        Options::read(&options(json!({ "max_chars": 0 }))).unwrap_err(),
        "`plugins.command.review.options.max_chars` must allow at least one character."
    );
}

#[test]
fn an_empty_dir_or_base_is_refused() {
    assert_eq!(
        Options::read(&options(json!({ "dir": " " }))).unwrap_err(),
        "`plugins.command.review.options.dir` is empty."
    );
    assert_eq!(
        Options::read(&options(json!({ "base": [] }))).unwrap_err(),
        "`plugins.command.review.options.base` needs a program to run, as in `[\"git\", \"show\", \
         \"HEAD:./{path}\"]`."
    );
}

#[test]
fn a_value_of_the_wrong_type_names_the_options() {
    let error = Options::read(&options(json!({ "base": "git show" }))).unwrap_err();

    assert!(
        error.starts_with("`plugins.command.review.options` is invalid: "),
        "{error}"
    );
}

#[test]
fn the_store_is_in_user_local_storage_unless_dir_is_set() {
    let root = Utf8Path::new("/work");
    let storage = Utf8Path::new("/data/jp/work-id");
    let unset = Options::default();
    let relative = Options {
        dir: Some(Utf8PathBuf::from(".jp/reviews")),
        ..Options::default()
    };
    let absolute = Options {
        dir: Some(Utf8PathBuf::from("/elsewhere/reviews")),
        ..Options::default()
    };

    assert_eq!(
        unset.store_dir(root, Some(storage)).unwrap(),
        "/data/jp/work-id/review"
    );
    assert_eq!(
        relative.store_dir(root, Some(storage)).unwrap(),
        "/work/.jp/reviews"
    );
    assert_eq!(
        absolute.store_dir(root, None).unwrap(),
        "/elsewhere/reviews"
    );
}

#[test]
fn without_user_local_storage_dir_has_to_be_set() {
    let error = Options::default()
        .store_dir(Utf8Path::new("/work"), None)
        .unwrap_err();

    assert_eq!(
        error,
        "This workspace has no user-local storage to keep reviews in; set \
         `plugins.command.review.options.dir` to a directory for them."
    );
}
