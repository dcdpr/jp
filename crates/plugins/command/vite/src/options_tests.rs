use jp_config::{
    PartialAppConfig,
    assignment::{AssignKeyValue as _, KvAssignment},
};
use serde_json::json;

use super::*;
use crate::site::FILES;

fn read(options: Value) -> Result<Options, String> {
    let Value::Object(options) = options else {
        panic!("options are an object");
    };

    Options::read(&options)
}

#[test]
fn every_option_has_a_default() {
    assert_eq!(
        read(json!({})),
        Ok(Options {
            dir: Utf8PathBuf::from("docs"),
            repository: None,
        })
    );
}

#[test]
fn configured_options_replace_the_defaults() {
    assert_eq!(
        read(json!({ "dir": "site", "repository": "https://github.com/dcdpr/jp/" })),
        Ok(Options {
            dir: Utf8PathBuf::from("site"),
            repository: Some("https://github.com/dcdpr/jp".to_owned()),
        })
    );
}

/// A configuration written for a newer `jp-vite` must still load.
#[test]
fn an_unknown_option_is_ignored() {
    assert_eq!(read(json!({ "theme": "dark" })), Ok(Options::default()));
}

#[test]
fn an_empty_directory_is_refused() {
    assert_eq!(
        read(json!({ "dir": " " })),
        Err("`plugins.command.vite.options.dir` is empty.".to_owned())
    );
}

/// Links to repository files are followed by a browser.
#[test]
fn the_repository_has_to_be_a_web_address() {
    for repository in [
        "git@github.com:dcdpr/jp.git",
        "github.com/dcdpr/jp",
        "https://",
        "",
    ] {
        assert_eq!(
            read(json!({ "repository": repository })),
            Err(
                "`plugins.command.vite.options.repository` must be an `https://` address."
                    .to_owned()
            ),
            "{repository}"
        );
    }
}

#[test]
fn the_options_are_read_from_the_whole_configuration() {
    assert_eq!(Options::from_config(&json!({})), Ok(Options::default()));
    assert_eq!(
        Options::from_config(&json!({
            "plugins": { "command": { "vite": { "options": { "dir": "site" } } } }
        }))
        .unwrap()
        .dir,
        "site"
    );
}

#[test]
fn the_rfd_options_default_to_what_jp_rfd_uses() {
    assert_eq!(
        rfd_options(&json!({})),
        Ok(RfdOptions {
            dir: Utf8PathBuf::from("docs/rfd"),
            categories: vec![
                "design".to_owned(),
                "decision".to_owned(),
                "guide".to_owned(),
                "process".to_owned(),
            ],
            tracking_key: "tracking-rfd".to_owned(),
            relevant_key: "relevant-rfd".to_owned(),
        })
    );
}

#[test]
fn the_rfd_options_are_read_from_jp_rfds_settings() {
    let options = rfd_options(&json!({
        "plugins": { "command": { "rfd": { "options": {
            "dir": "design",
            "categories": ["Design", "ADR", "design"],
            "tracking": { "key": "tracks", "labels": ["type=tracking"] },
            "relevant": { "key": "rfd" }
        } } } }
    }))
    .unwrap();

    assert_eq!(options.dir, "design");
    assert_eq!(options.categories, ["design", "adr"]);
    assert_eq!(options.tracking_key, "tracks");
    assert_eq!(options.relevant_key, "rfd");
}

#[test]
fn an_empty_category_list_is_refused() {
    assert_eq!(
        rfd_options(&json!({
            "plugins": { "command": { "rfd": { "options": { "categories": [" "] } } } }
        })),
        Err("`plugins.command.rfd.options.categories` is empty.".to_owned())
    );
}

#[test]
fn the_ticket_options_default_to_what_jp_ticket_uses() {
    assert_eq!(
        ticket_options(&json!({})),
        Ok(TicketOptions {
            dir: Utf8PathBuf::from("docs/ticket"),
            done_limit: 8,
            labels: Vocabulary::default(),
        })
    );
}

#[test]
fn the_ticket_options_are_read_from_jp_tickets_settings() {
    let options = ticket_options(&json!({
        "plugins": { "command": { "ticket": { "options": {
            "dir": "work",
            "board": { "done_limit": 3 },
            "labels": { "type": { "description": "The type", "values": ["bug"] } }
        } } } }
    }))
    .unwrap();

    assert_eq!(options.dir, "work");
    assert_eq!(options.done_limit, 3);
    assert_eq!(options.labels.tokens(), ["type=bug"]);
}

/// A vocabulary that wouldn't let `jp ticket` label anything would let the site
/// pass labels nothing defines.
#[test]
fn a_broken_vocabulary_is_named_by_where_it_is_configured() {
    let error = ticket_options(&json!({
        "plugins": { "command": { "ticket": { "options": { "labels": { "type": "x" } } } } }
    }))
    .unwrap_err();

    assert!(
        error.starts_with(
            "`plugins.command.ticket.options.labels`: The label vocabulary is not a map"
        ),
        "{error}"
    );
}

#[test]
fn an_option_of_the_wrong_type_is_named() {
    let error = ticket_options(&json!({
        "plugins": { "command": { "ticket": { "options": { "board": { "done_limit": "eight" } } } } }
    }))
    .unwrap_err();

    assert!(
        error.starts_with("`plugins.command.ticket.options` is invalid: invalid type: string"),
        "{error}"
    );
}

/// Every write from the dev server pins the directories the site shows with
/// `--cfg`.
/// `jp` applies each the way it applies any `--cfg`, so a key it doesn't know
/// would fail every write, and one landing anywhere else would leave the write
/// on the configured directory rather than the shown one.
#[test]
fn the_dev_server_pins_directories_where_the_plugins_read_them() {
    let (_, middleware) = FILES
        .iter()
        .find(|(path, _)| *path == "jp-vite/middleware.mjs")
        .unwrap();
    let middleware = std::str::from_utf8(middleware).unwrap();
    for template in [
        "`plugins.command.rfd.options.dir=${settings.rfd.dir}`",
        "`plugins.command.ticket.options.dir=${settings.ticket.dir}`",
    ] {
        assert!(
            middleware.contains(template),
            "middleware.mjs no longer passes {template}"
        );
    }

    let mut partial = PartialAppConfig::default();
    for arg in [
        "plugins.command.rfd.options.dir=site/design",
        "plugins.command.ticket.options.dir=site/work",
    ] {
        let assignment: KvAssignment = arg.parse().unwrap();
        partial.assign(assignment).unwrap();
    }
    let config = serde_json::to_value(&partial).unwrap();

    assert_eq!(rfd_options(&config).unwrap().dir, "site/design");
    assert_eq!(ticket_options(&config).unwrap().dir, "site/work");
}

#[test]
fn web_remotes_lose_their_credentials_and_suffix() {
    assert_eq!(
        web_address("https://github.com/dcdpr/jp.git").as_deref(),
        Some("https://github.com/dcdpr/jp")
    );
    assert_eq!(
        web_address("https://x-access-token:secret@github.com/dcdpr/jp").as_deref(),
        Some("https://github.com/dcdpr/jp")
    );
    assert_eq!(
        web_address("https://git.example.com:8443/team/site/").as_deref(),
        Some("https://git.example.com:8443/team/site")
    );
    assert_eq!(
        web_address("http://git.local/team/site").as_deref(),
        Some("http://git.local/team/site")
    );
}

/// Remotes reached over ssh or git still have a web side, on the same host.
#[test]
fn ssh_remotes_are_read_as_their_web_address() {
    assert_eq!(
        web_address("git@github.com:dcdpr/jp.git").as_deref(),
        Some("https://github.com/dcdpr/jp")
    );
    assert_eq!(
        web_address("ssh://git@github.com:22/dcdpr/jp.git").as_deref(),
        Some("https://github.com/dcdpr/jp")
    );
    assert_eq!(
        web_address("git://github.com/dcdpr/jp").as_deref(),
        Some("https://github.com/dcdpr/jp")
    );
}

#[test]
fn a_remote_without_a_web_side_has_no_address() {
    for remote in [
        "/srv/git/jp.git",
        "../jp",
        "file:///srv/git/jp.git",
        "C:\\repos\\jp",
        "https://github.com",
        "",
    ] {
        assert_eq!(web_address(remote), None, "{remote}");
    }
}
