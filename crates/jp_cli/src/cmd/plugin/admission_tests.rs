use camino::Utf8PathBuf;
use camino_tempfile::tempdir;
use jp_config::{
    AppConfig,
    providers::mcp::{AlgorithmConfig, ChecksumConfig},
    types::map::MergeableMap,
};
use jp_plugin::Manifest;
use pretty_assertions::assert_eq;

use super::*;
use crate::cmd::plugin::discovery::{Location, ManifestState};

fn plugin(name: &str) -> LocalPlugin {
    LocalPlugin {
        name: name.to_owned(),
        path: Utf8PathBuf::from(format!("/bin/jp-{name}")),
        location: Location::Path,
        manifest: ManifestState::Valid(Manifest {
            protocol: 1,
            description: "Web UI".to_owned(),
            command: vec!["serve".to_owned(), "web".to_owned()],
        }),
    }
}

/// A binary whose SHA-256 is `aaa`, which is also what a SHA-256 pin is
/// compared against.
fn candidate(plugin: &LocalPlugin) -> Candidate<'_> {
    Candidate {
        plugin,
        sha256: "aaa",
        pin_digest: Some("aaa"),
        official: false,
        official_sha256: None,
        replaces: None,
        approval: ApprovalMatch::None,
    }
}

fn config(run: Option<RunPolicy>, pinned: Option<&str>) -> CommandPluginConfig {
    CommandPluginConfig {
        run,
        checksum: pinned.map(|value| ChecksumConfig {
            algorithm: AlgorithmConfig::Sha256,
            value: value.to_owned(),
        }),
        options: MergeableMap::default(),
    }
}

#[test]
fn deny_refuses_whatever_else_would_admit_it() {
    let plugin = plugin("webui");
    let approved = Candidate {
        approval: ApprovalMatch::Matches,
        ..candidate(&plugin)
    };

    assert_eq!(
        decide(&approved, Some(&config(Some(RunPolicy::Deny), None))),
        Verdict::Refuse(
            "plugin `webui` is denied by configuration (plugins.command.webui.run = \"deny\")"
                .to_owned()
        )
    );
}

#[test]
fn a_pinned_checksum_refuses_a_binary_that_does_not_match_it_even_under_allow() {
    let plugin = plugin("webui");

    let verdict = decide(
        &candidate(&plugin),
        Some(&config(Some(RunPolicy::Allow), Some("bbb"))),
    );

    assert!(matches!(verdict, Verdict::Refuse(reason) if reason.contains("checksum mismatch")));
}

/// A pin with nothing to compare it against refuses rather than passes.
#[test]
fn a_pin_without_a_digest_refuses() {
    let plugin = plugin("webui");
    let unhashed = Candidate {
        pin_digest: None,
        ..candidate(&plugin)
    };

    let verdict = decide(
        &unhashed,
        Some(&config(Some(RunPolicy::Allow), Some("aaa"))),
    );

    assert!(matches!(verdict, Verdict::Refuse(reason) if reason.contains("checksum mismatch")));
}

/// A pin says which algorithm its value is in, and the binary is hashed with
/// that algorithm before the two are compared.
#[test]
fn a_sha1_pin_is_compared_with_the_binarys_sha1() {
    let tmp = tempdir().unwrap();
    let path = tmp.path().join("jp-webui");
    std::fs::write(&path, "hello").unwrap();
    let plugin = LocalPlugin {
        path: path.clone(),
        ..plugin("webui")
    };

    let pinned = |value: &str| {
        let mut plugins = AppConfig::new_test().plugins;
        plugins
            .command
            .insert("webui".to_owned(), CommandPluginConfig {
                run: Some(RunPolicy::Allow),
                checksum: Some(ChecksumConfig {
                    algorithm: AlgorithmConfig::Sha1,
                    value: value.to_owned(),
                }),
                options: MergeableMap::default(),
            });
        plugins
    };

    let mut approvals = ApprovalStore::load_from(Some(tmp.path().join("approvals.json")));
    let (printer, _out, _err) = Printer::memory(jp_printer::OutputFormat::Text);

    // The SHA-1 of "hello".
    let matching = pinned("aaf4c61ddcc5e8a2dabede0f3b482cd9aea9434d");
    assert_eq!(
        admit(
            &plugin,
            Official::default(),
            &matching,
            &mut approvals,
            false,
            &printer
        )
        .map_err(|error| error.message.unwrap_or_default()),
        Ok(())
    );

    let other = pinned("bbb");
    assert_eq!(
        admit(
            &plugin,
            Official::default(),
            &other,
            &mut approvals,
            false,
            &printer
        )
        .map_err(|error| error.message.unwrap_or_default()),
        Err(format!(
            "plugin `webui` binary checksum mismatch.\nexpected: bbb\nactual:   \
             aaf4c61ddcc5e8a2dabede0f3b482cd9aea9434d\nThe binary at {path} has changed since it \
             was pinned. Update plugins.command.webui.checksum.value in your config to accept the \
             new binary."
        ))
    );
}

#[test]
fn allow_runs_without_asking() {
    let plugin = plugin("webui");

    assert_eq!(
        decide(
            &candidate(&plugin),
            Some(&config(Some(RunPolicy::Allow), Some("aaa")))
        ),
        Verdict::Run
    );
}

/// Every plugin is at `ask` unless config says otherwise.
#[test]
fn ask_is_the_default() {
    let plugin = plugin("webui");

    assert_eq!(decide(&candidate(&plugin), None), Verdict::Ask(Reason::New));
}

#[test]
fn the_official_release_runs_without_asking() {
    let plugin = plugin("serve-web");
    let official = Candidate {
        official: true,
        official_sha256: Some("aaa"),
        ..candidate(&plugin)
    };

    assert_eq!(decide(&official, None), Verdict::Run);
}

/// A package manager's own build of an official plugin has other bytes than the
/// release, so the registry cannot vouch for it.
#[test]
fn an_official_binary_that_is_not_the_release_is_asked_about() {
    let plugin = plugin("serve-web");
    let official = Candidate {
        official: true,
        official_sha256: Some("the release"),
        ..candidate(&plugin)
    };

    assert_eq!(decide(&official, None), Verdict::Ask(Reason::NotTheRelease));
}

#[test]
fn an_approved_binary_runs_without_asking() {
    let plugin = plugin("webui");
    let approved = Candidate {
        approval: ApprovalMatch::Matches,
        ..candidate(&plugin)
    };

    assert_eq!(decide(&approved, None), Verdict::Run);
}

#[test]
fn a_changed_or_moved_binary_is_asked_about_again() {
    let plugin = plugin("webui");

    let changed = Candidate {
        approval: ApprovalMatch::Changed,
        ..candidate(&plugin)
    };
    assert_eq!(decide(&changed, None), Verdict::Ask(Reason::Changed));

    let elsewhere = Candidate {
        approval: ApprovalMatch::Elsewhere("/other/jp-webui".into()),
        ..candidate(&plugin)
    };
    assert_eq!(
        decide(&elsewhere, None),
        Verdict::Ask(Reason::Elsewhere("/other/jp-webui".into()))
    );
}

#[test]
fn the_prompt_names_a_third_party_plugin_replacing_an_official_command() {
    let plugin = plugin("webui");
    let replacing = Candidate {
        replaces: Some("serve web"),
        ..candidate(&plugin)
    };

    assert_eq!(
        question_lines(&replacing, &Reason::New, |word| format!("**{word}**")),
        [
            "`jp serve web` is claimed by the **third-party** plugin `webui`, which replaces the \
             official one.",
            "/bin/jp-webui",
        ]
    );
}

#[test]
fn the_prompt_says_what_a_new_third_party_plugin_does() {
    let plugin = plugin("webui");

    assert_eq!(
        question_lines(&candidate(&plugin), &Reason::New, ToOwned::to_owned),
        [
            "`jp serve web` is provided by the third-party plugin `webui`: Web UI",
            "/bin/jp-webui",
        ]
    );
}

#[test]
fn the_prompt_says_the_binary_changed_or_names_the_approved_one() {
    let plugin = plugin("webui");

    assert_eq!(
        question_lines(&candidate(&plugin), &Reason::Changed, ToOwned::to_owned)[2],
        "It changed since you approved it."
    );
    assert_eq!(
        question_lines(
            &candidate(&plugin),
            &Reason::Elsewhere("/usr/bin/jp-webui".into()),
            ToOwned::to_owned
        )[2],
        "You approved `webui` at /usr/bin/jp-webui, not this file."
    );
}

#[test]
fn the_prompt_says_an_official_binary_is_not_the_release() {
    let plugin = plugin("serve-web");
    let official = Candidate {
        official: true,
        ..candidate(&plugin)
    };

    assert_eq!(
        question_lines(&official, &Reason::NotTheRelease, ToOwned::to_owned)[0],
        "`jp serve web` is provided by the official plugin `serve-web`, but this binary is not \
         the release the registry publishes."
    );
}

/// The whole path without a terminal: a binary nobody approved is refused, the
/// same file once approved runs, and the file changed after that is refused
/// again.
#[test]
fn without_a_terminal_only_an_approved_binary_runs() {
    let tmp = tempdir().unwrap();
    let path = tmp.path().join("jp-webui");
    std::fs::write(&path, "v1").unwrap();
    let plugin = LocalPlugin {
        path: path.clone(),
        ..plugin("webui")
    };

    let config = AppConfig::new_test().plugins;
    let mut approvals = ApprovalStore::load_from(Some(tmp.path().join("approvals.json")));
    let (printer, _out, _err) = Printer::memory(jp_printer::OutputFormat::Text);
    let run = |approvals: &mut ApprovalStore| {
        admit(
            &plugin,
            Official::default(),
            &config,
            approvals,
            false,
            &printer,
        )
        .map_err(|error| error.message.unwrap_or_default())
    };

    assert_eq!(
        run(&mut approvals),
        Err(format!(
            "plugin `webui` at {path} is not approved. Approve it with `jp plugin approve \
             {path}`, or set plugins.command.webui.run = \"allow\" in config."
        ))
    );

    approvals
        .record("webui", ApprovedPlugin {
            path: path.clone(),
            sha256: registry::sha256_file(&path).unwrap(),
            approved_at: Utc::now(),
            installed: false,
            manifest: None,
        })
        .unwrap();
    assert_eq!(run(&mut approvals), Ok(()));

    std::fs::write(&path, "v2").unwrap();
    assert_eq!(
        run(&mut approvals),
        Err(format!(
            "plugin `webui` at {path} is not approved (it changed since it was approved). Approve \
             it with `jp plugin approve {path}`, or set plugins.command.webui.run = \"allow\" in \
             config."
        ))
    );
}

#[test]
fn without_a_terminal_the_refusal_says_how_to_approve() {
    let plugin = plugin("webui");

    assert_eq!(
        not_approved(&candidate(&plugin), &Reason::Changed),
        "plugin `webui` at /bin/jp-webui is not approved (it changed since it was approved). \
         Approve it with `jp plugin approve /bin/jp-webui`, or set plugins.command.webui.run = \
         \"allow\" in config."
    );
}
