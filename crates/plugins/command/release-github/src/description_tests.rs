use changelog::{Credit, CreditKind, Fragment, Type, Upgrade, release::Credited};
use indoc::indoc;

use super::*;

const DOWNLOAD: &str = "https://github.com/dcdpr/jp/releases/download/v0.3.0";

fn pull_request(number: u64) -> Credit {
    Credit {
        kind: CreditKind::ChangeRequest,
        label: format!("#{number}"),
        url: Some(format!("https://github.com/dcdpr/jp/pull/{number}")),
    }
}

fn fix() -> Credited<Fragment> {
    Credited {
        slug: "retry-idle-turns".to_owned(),
        credits: vec![pull_request(1207)],
        file: Fragment {
            type_: Type::Fixed,
            credits: None,
            entry: "A turn that stalls while idle is retried instead of failing.".to_owned(),
            upgrade: None,
        },
    }
}

fn breaking_change() -> Credited<Fragment> {
    Credited {
        slug: "plugin-manifest-routing".to_owned(),
        credits: vec![pull_request(1216)],
        file: Fragment {
            type_: Type::Changed,
            credits: None,
            entry: "JP finds a command plugin by the manifest its binary embeds.".to_owned(),
            upgrade: Some(Upgrade {
                title: "Command plugins are found by their manifest".to_owned(),
                body: "Rebuild the plugin, or reinstall it.".to_owned(),
            }),
        },
    }
}

fn release(entries: Vec<Credited<Fragment>>) -> Release {
    Release {
        version: Some(Version::new(0, 3, 0)),
        date: Some("2026-11-11".to_owned()),
        overview: "JP 0.3.0 changes how plugins are found.".to_owned(),
        highlights: vec![],
        entries,
    }
}

fn headings() -> Headings {
    let mut headings = Headings::default();
    headings.set(Type::Fixed, "Bug fixes");

    headings
}

#[test]
fn a_release_without_breaking_changes_is_its_notes() {
    let text = description(
        &release(vec![fix()]),
        &headings(),
        &Version::new(0, 3, 0),
        Some("https://jp.computer/releases/{version}/upgrade"),
        None,
        DOWNLOAD,
    );

    assert_eq!(text, indoc! {"
        JP 0.3.0 changes how plugins are found.

        ## Changelog

        ### Bug fixes

        - A turn that stalls while idle is retried instead of failing.
          ([#1207](https://github.com/dcdpr/jp/pull/1207))
    "});
}

/// With an address for the guide, breaking entries link to the release's own
/// guide there, and the description leaves the guide out.
#[test]
fn breaking_entries_link_to_the_upgrade_guide_at_its_address() {
    let text = description(
        &release(vec![breaking_change(), fix()]),
        &headings(),
        &Version::new(0, 3, 0),
        Some("https://jp.computer/releases/{version}/upgrade"),
        None,
        DOWNLOAD,
    );

    assert_eq!(text, indoc! {"
        JP 0.3.0 changes how plugins are found.

        ## Changelog

        ### Changed

        - JP finds a command plugin by the manifest its binary embeds.
          **Breaking**: see [the upgrade guide](https://jp.computer/releases/0.3.0/upgrade#command-plugins-are-found-by-their-manifest).
          ([#1216](https://github.com/dcdpr/jp/pull/1216))

        ### Bug fixes

        - A turn that stalls while idle is retried instead of failing.
          ([#1207](https://github.com/dcdpr/jp/pull/1207))
    "});
}

/// Without an address, the guide follows the notes, so nothing a user has to do
/// is missing from the Release.
#[test]
fn without_an_address_the_upgrade_guide_follows_the_notes() {
    let text = description(
        &release(vec![breaking_change()]),
        &headings(),
        &Version::new(0, 3, 0),
        None,
        None,
        DOWNLOAD,
    );

    assert_eq!(text, indoc! {"
        JP 0.3.0 changes how plugins are found.

        ## Changelog

        ### Changed

        - JP finds a command plugin by the manifest its binary embeds.
          **Breaking**: see [the upgrade guide](#command-plugins-are-found-by-their-manifest).
          ([#1216](https://github.com/dcdpr/jp/pull/1216))

        # Upgrade guide

        ## Command plugins are found by their manifest

        [#1216](https://github.com/dcdpr/jp/pull/1216)

        Rebuild the plugin, or reinstall it.
    "});
}

#[test]
fn the_footer_ends_the_description_with_the_release_filled_in() {
    let footer = indoc! {"
        ## Install

        ```sh
        curl -LsSf {download}/install.sh | sh
        ```

        This installs {version}.

    "};

    let text = description(
        &release(vec![fix()]),
        &headings(),
        &Version::new(0, 3, 0),
        None,
        Some(footer),
        DOWNLOAD,
    );

    assert_eq!(text, indoc! {"
        JP 0.3.0 changes how plugins are found.

        ## Changelog

        ### Bug fixes

        - A turn that stalls while idle is retried instead of failing.
          ([#1207](https://github.com/dcdpr/jp/pull/1207))

        ## Install

        ```sh
        curl -LsSf https://github.com/dcdpr/jp/releases/download/v0.3.0/install.sh | sh
        ```

        This installs 0.3.0.
    "});
}

#[test]
fn a_description_edited_on_github_reads_the_same_as_its_rendering() {
    assert!(same(
        "Notes.\r\n\r\n- An entry.\r\n",
        "Notes.\n\n- An entry.\n"
    ));
    assert!(same("Notes.", "Notes.\n"));
    assert!(!same("Notes.", "Other notes.\n"));
}
