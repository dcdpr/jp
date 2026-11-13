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
        shutdown_timeout_secs: None,
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
        decide(
            &approved,
            Some(&config(Some(RunPolicy::Deny), None)),
            RunPolicy::Ask
        ),
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
        RunPolicy::Ask,
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
        RunPolicy::Ask,
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
                shutdown_timeout_secs: None,
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
        .map(drop)
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
        .map(drop)
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
            Some(&config(Some(RunPolicy::Allow), Some("aaa"))),
            RunPolicy::Ask
        ),
        Verdict::Run
    );
}

/// `plugins.command.'*'.run` is the policy of a plugin that sets none, whether
/// or not the plugin is configured at all.
#[test]
fn a_plugin_without_its_own_policy_gets_the_default() {
    let plugin = plugin("webui");

    assert_eq!(
        decide(&candidate(&plugin), None, RunPolicy::Allow),
        Verdict::Run
    );
    assert_eq!(
        decide(
            &candidate(&plugin),
            Some(&config(None, None)),
            RunPolicy::Allow
        ),
        Verdict::Run
    );
}

/// The refusal names the key that denies, which is not one the plugin has.
#[test]
fn a_default_of_deny_refuses_and_names_the_default() {
    let plugin = plugin("webui");
    let approved = Candidate {
        approval: ApprovalMatch::Matches,
        ..candidate(&plugin)
    };

    assert_eq!(
        decide(&approved, None, RunPolicy::Deny),
        Verdict::Refuse(
            "plugin `webui` is denied by configuration (plugins.command.'*'.run = \"deny\")"
                .to_owned()
        )
    );
}

#[test]
fn a_plugins_own_policy_overrides_the_default() {
    let plugin = plugin("webui");

    // Allowed by name where everything else is denied.
    assert_eq!(
        decide(
            &candidate(&plugin),
            Some(&config(Some(RunPolicy::Allow), None)),
            RunPolicy::Deny
        ),
        Verdict::Run
    );

    // Asked about where everything else runs.
    assert_eq!(
        decide(
            &candidate(&plugin),
            Some(&config(Some(RunPolicy::Ask), None)),
            RunPolicy::Allow
        ),
        Verdict::Ask(Reason::New)
    );

    // Denied by name where everything else runs, and the refusal names the
    // plugin's own key.
    assert_eq!(
        decide(
            &candidate(&plugin),
            Some(&config(Some(RunPolicy::Deny), None)),
            RunPolicy::Allow
        ),
        Verdict::Refuse(
            "plugin `webui` is denied by configuration (plugins.command.webui.run = \"deny\")"
                .to_owned()
        )
    );
}

/// A default of `allow` trusts a plugin by name, as its own `allow` does, and a
/// pinned checksum still decides which contents that name may have.
#[test]
fn a_pinned_checksum_refuses_a_mismatch_under_a_default_of_allow() {
    let plugin = plugin("webui");

    assert_eq!(
        decide(
            &candidate(&plugin),
            Some(&config(None, Some("bbb"))),
            RunPolicy::Allow
        ),
        Verdict::Refuse(
            "plugin `webui` binary checksum mismatch.\nexpected: bbb\nactual:   aaa\nThe binary \
             at /bin/jp-webui has changed since it was pinned. Update \
             plugins.command.webui.checksum.value in your config to accept the new binary."
                .to_owned()
        )
    );
}

/// `admit` reads the default from the configuration it is given.
#[test]
fn admit_applies_the_default_policy_of_the_configuration() {
    let tmp = tempdir().unwrap();
    let path = tmp.path().join("jp-webui");
    std::fs::write(&path, "v1").unwrap();
    let plugin = LocalPlugin {
        path,
        ..plugin("webui")
    };

    let mut approvals = ApprovalStore::load_from(Some(tmp.path().join("approvals.json")));
    let (printer, _out, _err) = Printer::memory(jp_printer::OutputFormat::Text);
    let mut admit_under = |default: RunPolicy| {
        let mut config = AppConfig::new_test().plugins;
        config.command.defaults.run = default;

        admit(
            &plugin,
            Official::default(),
            &config,
            &mut approvals,
            false,
            &printer,
        )
        .map(drop)
        .map_err(|error| error.message.unwrap_or_default())
    };

    assert_eq!(
        admit_under(RunPolicy::Deny),
        Err(
            "plugin `webui` is denied by configuration (plugins.command.'*'.run = \"deny\")"
                .to_owned()
        )
    );
    // Nobody approved the binary, so it runs only because the default allows.
    assert_eq!(admit_under(RunPolicy::Allow), Ok(()));
}

/// Under `ask`, a binary nothing vouches for is asked about.
#[test]
fn a_new_binary_is_asked_about_under_ask() {
    let plugin = plugin("webui");

    assert_eq!(
        decide(&candidate(&plugin), None, RunPolicy::Ask),
        Verdict::Ask(Reason::New)
    );
}

#[test]
fn the_official_release_runs_without_asking() {
    let plugin = plugin("serve-web");
    let official = Candidate {
        official: true,
        official_sha256: Some("aaa"),
        ..candidate(&plugin)
    };

    assert_eq!(decide(&official, None, RunPolicy::Ask), Verdict::Run);
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

    assert_eq!(
        decide(&official, None, RunPolicy::Ask),
        Verdict::Ask(Reason::NotTheRelease)
    );
}

#[test]
fn an_approved_binary_runs_without_asking() {
    let plugin = plugin("webui");
    let approved = Candidate {
        approval: ApprovalMatch::Matches,
        ..candidate(&plugin)
    };

    assert_eq!(decide(&approved, None, RunPolicy::Ask), Verdict::Run);
}

#[test]
fn a_changed_or_moved_binary_is_asked_about_again() {
    let plugin = plugin("webui");

    let changed = Candidate {
        approval: ApprovalMatch::Changed,
        ..candidate(&plugin)
    };
    assert_eq!(
        decide(&changed, None, RunPolicy::Ask),
        Verdict::Ask(Reason::Changed)
    );

    let elsewhere = Candidate {
        approval: ApprovalMatch::Elsewhere("/other/jp-webui".into()),
        ..candidate(&plugin)
    };
    assert_eq!(
        decide(&elsewhere, None, RunPolicy::Ask),
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
        .map(drop)
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

/// Admission hands back the digest it decided on, so a caller that pins the
/// binary for later checks pins the approved contents, not whatever is on disk
/// by the time it reads the file again.
#[test]
fn admission_returns_the_digest_it_decided_on() {
    let tmp = tempdir().unwrap();
    let path = tmp.path().join("jp-webui");
    std::fs::write(&path, "v1").unwrap();
    let plugin = LocalPlugin {
        path: path.clone(),
        ..plugin("webui")
    };
    let approved = registry::sha256_file(&path).unwrap();
    let mut approvals = ApprovalStore::load_from(Some(tmp.path().join("approvals.json")));
    approvals
        .record("webui", ApprovedPlugin {
            path: path.clone(),
            sha256: approved.clone(),
            approved_at: Utc::now(),
            installed: false,
            manifest: None,
        })
        .unwrap();
    let (printer, _out, _err) = Printer::memory(jp_printer::OutputFormat::Text);

    let decided = admit(
        &plugin,
        Official::default(),
        &AppConfig::new_test().plugins,
        &mut approvals,
        false,
        &printer,
    )
    .map_err(|error| error.message.unwrap_or_default());

    assert_eq!(decided, Ok(approved));
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
