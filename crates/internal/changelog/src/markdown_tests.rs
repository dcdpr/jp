use indoc::indoc;
use semver::Version;

use super::*;
use crate::{fragment::Upgrade, highlight::Highlight};

fn pull_request(number: u64) -> Credit {
    Credit {
        kind: CreditKind::ChangeRequest,
        label: format!("#{number}"),
        url: Some(format!("https://github.com/dcdpr/jp/pull/{number}")),
    }
}

fn entry(slug: &str, type_: Type, entry: &str, credits: Vec<Credit>) -> Credited<Fragment> {
    Credited {
        slug: slug.to_owned(),
        credits,
        file: Fragment {
            type_,
            credits: None,
            entry: entry.to_owned(),
            upgrade: None,
        },
    }
}

fn breaking(
    slug: &str,
    type_: Type,
    text: &str,
    credits: Vec<Credit>,
    title: &str,
    body: &str,
) -> Credited<Fragment> {
    let mut breaking = entry(slug, type_, text, credits);
    breaking.file.upgrade = Some(Upgrade {
        title: title.to_owned(),
        body: body.to_owned(),
    });

    breaking
}

fn release(entries: Vec<Credited<Fragment>>) -> Release {
    Release {
        version: Some(Version::new(0, 3, 0)),
        date: None,
        overview: "JP 0.3.0 calls tools on many inputs.".to_owned(),
        highlights: vec![],
        entries,
    }
}

fn jp_0_3_0() -> Release {
    Release {
        version: Some(Version::new(0, 3, 0)),
        date: Some("2026-10-28".to_owned()),
        overview: "JP 0.3.0 lets the assistant call one tool on many inputs at once.".to_owned(),
        highlights: vec![Credited {
            slug: "fan-out".to_owned(),
            credits: vec![pull_request(1182), pull_request(1183)],
            file: Highlight {
                title: "Tool calls fan out".to_owned(),
                authors: vec!["@JeanMertz".to_owned()],
                credits: None,
                body: "When the assistant needs the same tool on several inputs, it asks once."
                    .to_owned(),
            },
        }],
        entries: vec![
            entry(
                "fan-out",
                Type::Added,
                "The assistant can call one tool on many inputs in a single request.",
                vec![pull_request(1182)],
            ),
            entry(
                "show-model",
                Type::Added,
                "`jp conversation show` shows the conversation's model.",
                vec![pull_request(1243)],
            ),
            breaking(
                "plugin-manifest-routing",
                Type::Changed,
                "JP finds a command plugin by the manifest its binary embeds, not by its file \
                 name.",
                vec![pull_request(1216)],
                "Command plugins are found by their manifest",
                "Rebuild the plugin against 0.3.0, or reinstall it:\n\n```sh\njp plugin install \
                 <name>\n```",
            ),
            entry(
                "retry-idle-turns",
                Type::Fixed,
                "A turn that stalls while idle is retried instead of failing.",
                vec![pull_request(1207)],
            ),
            entry(
                "help-default",
                Type::Fixed,
                "The help for `jp query --tmp` names the right default.",
                vec![Credit {
                    kind: CreditKind::Commit,
                    label: "4e1f0a2".to_owned(),
                    url: Some("https://github.com/dcdpr/jp/commit/4e1f0a2c9d8b".to_owned()),
                }],
            ),
        ],
    }
}

#[test]
fn the_release_notes_read_as_the_rfd_shows_them() {
    let headings = Headings::from_iter([
        (Type::Added, "New features".to_owned()),
        (Type::Fixed, "Bug fixes".to_owned()),
    ]);

    assert_eq!(notes(&jp_0_3_0(), &headings, UPGRADE_GUIDE), indoc! {"
            JP 0.3.0 lets the assistant call one tool on many inputs at once.

            ## Tool calls fan out

            When the assistant needs the same tool on several inputs, it asks once.

            By @JeanMertz in [#1182](https://github.com/dcdpr/jp/pull/1182) and [#1183](https://github.com/dcdpr/jp/pull/1183).

            ## Changelog

            ### New features

            - The assistant can call one tool on many inputs in a single request.
              ([#1182](https://github.com/dcdpr/jp/pull/1182))
            - `jp conversation show` shows the conversation's model.
              ([#1243](https://github.com/dcdpr/jp/pull/1243))

            ### Changed

            - JP finds a command plugin by the manifest its binary embeds, not by its file name.
              **Breaking**: see [the upgrade guide](upgrade#command-plugins-are-found-by-their-manifest).
              ([#1216](https://github.com/dcdpr/jp/pull/1216))

            ### Bug fixes

            - A turn that stalls while idle is retried instead of failing.
              ([#1207](https://github.com/dcdpr/jp/pull/1207))
            - The help for `jp query --tmp` names the right default.
              ([`4e1f0a2`](https://github.com/dcdpr/jp/commit/4e1f0a2c9d8b))
        "});
}

#[test]
fn the_upgrade_guide_has_a_section_for_each_breaking_entry() {
    let release = release(vec![
        breaking(
            "plugin-manifest-routing",
            Type::Changed,
            "JP finds a command plugin by its manifest.",
            vec![pull_request(1216)],
            "Command plugins are found by their manifest",
            "Rebuild the plugin against 0.3.0, or reinstall it:\n\n```sh\njp plugin install \
             <name>\n```",
        ),
        entry(
            "retry-idle-turns",
            Type::Fixed,
            "Idle turns are retried.",
            vec![pull_request(1207)],
        ),
        breaking(
            "enable-explicit",
            Type::Removed,
            "`enable` no longer accepts `\"explicit\"`.",
            vec![pull_request(1230), pull_request(1231)],
            "`enable = \"explicit\"` is no longer accepted",
            "Replace it with the table form.",
        ),
    ]);

    assert_eq!(upgrade_guide(&release), indoc! {r#"
        ## Command plugins are found by their manifest

        [#1216](https://github.com/dcdpr/jp/pull/1216)

        Rebuild the plugin against 0.3.0, or reinstall it:

        ```sh
        jp plugin install <name>
        ```

        ## `enable = "explicit"` is no longer accepted

        [#1230](https://github.com/dcdpr/jp/pull/1230), [#1231](https://github.com/dcdpr/jp/pull/1231)

        Replace it with the table form.
    "#});
}

#[test]
fn a_release_without_upgrade_sections_has_no_upgrade_guide() {
    let release = release(vec![entry(
        "retry-idle-turns",
        Type::Fixed,
        "Idle turns are retried.",
        vec![pull_request(1207)],
    )]);

    assert_eq!(upgrade_guide(&release), "");
}

/// The site numbers a heading whose anchor an earlier heading took, a heading
/// in a section's body included.
#[test]
fn each_breaking_entry_links_to_the_anchor_its_section_gets() {
    let release = release(vec![
        breaking(
            "first",
            Type::Changed,
            "The first change.",
            vec![],
            "Plugins move",
            "### Plugins move\n\nA heading in the body takes `plugins-move-1`.",
        ),
        breaking(
            "second",
            Type::Removed,
            "The second change.",
            vec![],
            "Plugins move",
            "Nothing more.",
        ),
    ]);

    let notes = notes(&release, &Headings::default(), "/releases/0.3.0/upgrade");

    assert!(
        notes.contains("[the upgrade guide](/releases/0.3.0/upgrade#plugins-move)."),
        "{notes}"
    );
    assert!(
        notes.contains("[the upgrade guide](/releases/0.3.0/upgrade#plugins-move-2)."),
        "{notes}"
    );
}

/// An entry of several blocks stays in its list item, and its credits follow a
/// code block as a paragraph of their own.
#[test]
fn an_entry_of_several_blocks_stays_in_its_item() {
    let release = release(vec![entry(
        "fan-out",
        Type::Added,
        "The assistant calls one tool on many inputs.\n\nFor example:\n\n```toml\nfan_out = \
         true\n```",
        vec![pull_request(1182)],
    )]);

    let notes = notes(&release, &Headings::default(), UPGRADE_GUIDE);

    assert!(
        notes.ends_with(indoc! {"
            ### Added

            - The assistant calls one tool on many inputs.

              For example:

              ```toml
              fan_out = true
              ```

              ([#1182](https://github.com/dcdpr/jp/pull/1182))
        "}),
        "{notes}"
    );
}

#[test]
fn a_credit_without_a_page_is_its_label() {
    let mut release = release(vec![entry(
        "retry-idle-turns",
        Type::Fixed,
        "Idle turns are retried.",
        vec![Credit {
            kind: CreditKind::Commit,
            label: "4e1f0a2".to_owned(),
            url: None,
        }],
    )]);
    release.highlights.push(Credited {
        slug: "retry".to_owned(),
        credits: vec![],
        file: Highlight {
            title: "Turns recover".to_owned(),
            authors: vec!["@JeanMertz".to_owned(), "@jdoe".to_owned()],
            credits: None,
            body: "A stalled turn is retried.".to_owned(),
        },
    });

    assert_eq!(
        notes(&release, &Headings::default(), UPGRADE_GUIDE),
        indoc! {"
            JP 0.3.0 calls tools on many inputs.

            ## Turns recover

            A stalled turn is retried.

            By @JeanMertz and @jdoe.

            ## Changelog

            ### Fixed

            - Idle turns are retried.
              (`4e1f0a2`)
        "}
    );
}

#[test]
fn a_release_without_entries_is_its_overview() {
    assert_eq!(
        notes(&release(vec![]), &Headings::default(), UPGRADE_GUIDE),
        "JP 0.3.0 calls tools on many inputs.\n"
    );
}
