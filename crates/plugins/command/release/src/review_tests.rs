use changelog::{Fragment, Highlight, Type, Upgrade, directory::Filed};
use indoc::indoc;
use jp_config::{
    PartialAppConfig,
    assignment::{AssignKeyValue as _, KvAssignment},
    assistant::tool_choice::ToolChoice,
    model::parameters::PartialReasoningConfig,
};

use super::*;

/// A fragment filed at `path`, with no upgrade section.
fn fragment(path: &str, type_: Type, entry: &str) -> Filed<Fragment> {
    Filed {
        slug: path
            .rsplit('/')
            .next()
            .unwrap()
            .trim_end_matches(".md")
            .to_owned(),
        path: path.to_owned(),
        file: Fragment {
            type_,
            change_requests: None,
            entry: entry.to_owned(),
            upgrade: None,
        },
    }
}

/// Release 0.3.0 with an overview, a highlight, and two entries, one of which
/// breaks something.
fn release_0_3_0() -> Subject {
    let mut breaking = fragment(
        "changelog.d/0.3.0/plugin-manifest-routing.md",
        Type::Changed,
        "JP finds a command plugin by the manifest its binary embeds.\n",
    );
    breaking.file.upgrade = Some(Upgrade {
        title: "Command plugins are found by their manifest".to_owned(),
        body: "Rebuild the plugin against 0.3.0.\n".to_owned(),
    });

    Subject {
        version: Some(Version::new(0, 3, 0)),
        overview: Some((
            "changelog.d/0.3.0/overview.md".to_owned(),
            "JP 0.3.0 lets the assistant call one tool on many inputs at once.\n".to_owned(),
        )),
        contents: Contents {
            highlights: vec![Filed {
                slug: "fan-out".to_owned(),
                path: "changelog.d/0.3.0/highlights/fan-out.md".to_owned(),
                file: Highlight {
                    title: "Tool calls fan out".to_owned(),
                    authors: vec!["@JeanMertz".to_owned()],
                    change_requests: None,
                    body: "JP runs the calls side by side.\n".to_owned(),
                },
            }],
            fragments: vec![
                breaking,
                fragment(
                    "changelog.d/0.3.0/retry-idle-turns.md",
                    Type::Fixed,
                    "Refactor the retry loop.",
                ),
            ],
        },
    }
}

fn finding(file: &str, kind: &str, quote: &str, comment: &str) -> Finding {
    Finding {
        file: file.to_owned(),
        kind: kind.to_owned(),
        quote: quote.to_owned(),
        comment: comment.to_owned(),
    }
}

/// The review runs in a conversation that is kept out of the workspace and
/// expires, with no reasoning and no tool calls, and reads every part of the
/// release people wrote, each tagged with its file.
#[test]
fn the_review_query_asks_for_structured_findings_in_a_throwaway_conversation() {
    let query = query(&release_0_3_0());

    assert!(query.new);
    assert_eq!(query.expires_in.as_deref(), Some("5m"));
    assert_eq!(query.title.as_deref(), Some("Review of release 0.3.0"));
    assert_eq!(query.cfg, [
        "conversation.start_local=true",
        "assistant.model.parameters.reasoning=off",
        "assistant.tool_choice=none",
    ]);
    assert_eq!(
        query.content.strip_prefix(PROMPT),
        Some(indoc! {r#"


            <release version="0.3.0">
            <overview file="changelog.d/0.3.0/overview.md">
            JP 0.3.0 lets the assistant call one tool on many inputs at once.
            </overview>
            <highlight file="changelog.d/0.3.0/highlights/fan-out.md">
            ## Tool calls fan out

            JP runs the calls side by side.
            </highlight>
            <entry file="changelog.d/0.3.0/plugin-manifest-routing.md" type="changed">
            JP finds a command plugin by the manifest its binary embeds.
            <upgrade>
            ## Command plugins are found by their manifest

            Rebuild the plugin against 0.3.0.
            </upgrade>
            </entry>
            <entry file="changelog.d/0.3.0/retry-idle-turns.md" type="fixed">
            Refactor the retry loop.
            </entry>
            </release>
        "#})
    );
}

/// An upgrade section that only names what broke reaches the model as a bare
/// heading, which is what a section without steps looks like.
#[test]
fn an_upgrade_section_without_steps_is_given_as_its_title() {
    let mut entry = fragment(
        "changelog.d/drop-legacy-config.md",
        Type::Removed,
        "The legacy config format is gone.",
    );
    entry.file.upgrade = Some(Upgrade {
        title: "The legacy config format is no longer read".to_owned(),
        body: String::new(),
    });
    let subject = Subject {
        version: None,
        overview: None,
        contents: Contents {
            fragments: vec![entry],
            highlights: vec![],
        },
    };

    assert!(query(&subject).content.ends_with(indoc! {r#"
        <entry file="changelog.d/drop-legacy-config.md" type="removed">
        The legacy config format is gone.
        <upgrade>
        ## The legacy config format is no longer read
        </upgrade>
        </entry>
        </release>
    "#}));
}

#[test]
fn the_unreleased_fragments_are_reviewed_under_their_own_name() {
    let subject = Subject {
        version: None,
        overview: None,
        contents: Contents {
            fragments: vec![fragment(
                "changelog.d/retry-idle-turns.md",
                Type::Fixed,
                "A turn that stalls while idle is retried.",
            )],
            highlights: vec![],
        },
    };

    let query = query(&subject);

    assert_eq!(
        query.title.as_deref(),
        Some("Review of the unreleased changes")
    );
    assert!(
        query
            .content
            .contains("\n\n<release version=\"unreleased\">\n<entry "),
        "{}",
        query.content
    );
    assert_eq!(subject.name(), "the unreleased fragments");
    assert_eq!(
        release_0_3_0().name(),
        "the notes and upgrade guide of 0.3.0"
    );
}

#[test]
fn a_subject_without_entries_highlights_or_overview_is_empty() {
    let empty = Subject {
        version: None,
        overview: None,
        contents: Contents::default(),
    };

    assert!(empty.is_empty());
    assert!(!release_0_3_0().is_empty());
}

/// `jp` applies each of the review query's `cfg` strings the way it applies
/// `--cfg`, so a key it doesn't know fails every review.
#[test]
fn the_review_query_configuration_names_keys_jp_knows() {
    let mut partial = PartialAppConfig::default();
    for arg in CFG {
        let assignment: KvAssignment = arg.parse().unwrap();
        partial.assign(assignment).unwrap();
    }

    assert_eq!(partial.conversation.start_local, Some(true));
    assert_eq!(
        partial.assistant.model.parameters.reasoning,
        Some(PartialReasoningConfig::Off)
    );
    assert_eq!(partial.assistant.tool_choice, Some(ToolChoice::None));
}

/// Each finding names its file, which kind of problem it is, the words it is
/// in, and how to fix them.
#[test]
fn the_schema_asks_for_findings_that_name_their_file() {
    let schema = Value::Object(query(&release_0_3_0()).schema.unwrap());
    let item = &schema["properties"]["findings"]["items"];

    assert_eq!(schema["required"], json!(["findings"]));
    assert_eq!(
        item["required"],
        json!(["file", "kind", "quote", "comment"])
    );
    assert_eq!(
        item["properties"]["kind"]["enum"],
        json!([
            "commit_message",
            "no_user_impact",
            "no_upgrade_steps",
            "typo"
        ])
    );
}

#[test]
fn the_findings_are_read_from_the_structured_response() {
    let read = findings(Some(json!({
        "findings": [{
            "file": "changelog.d/retry-idle-turns.md",
            "kind": "typo",
            "quote": "retired",
            "comment": "Write `retried`."
        }]
    })));

    assert_eq!(
        read,
        Ok(vec![finding(
            "changelog.d/retry-idle-turns.md",
            "typo",
            "retired",
            "Write `retried`."
        )])
    );
    assert_eq!(
        findings(None),
        Err("the response carried no structured data".to_owned())
    );

    let reason = findings(Some(json!({ "problems": [] }))).unwrap_err();
    assert!(
        reason.starts_with("the response didn't match the schema: "),
        "{reason}"
    );
}

/// Not every provider holds a response to the schema's `enum`, so a kind the
/// review doesn't name is kept rather than losing the whole review.
#[test]
fn a_kind_the_schema_does_not_name_is_kept_and_shown_as_written() {
    let read = findings(Some(json!({
        "findings": [{
            "file": "changelog.d/x.md",
            "kind": "wrong_type",
            "quote": "Fixed a crash.",
            "comment": "File it as `fixed`."
        }]
    })))
    .unwrap();

    assert_eq!(read[0].kind, "wrong_type");
    assert_eq!(report(&read, "the unreleased fragments"), indoc! {r#"
            changelog.d/x.md
            - wrong_type: "Fixed a crash."
              File it as `fixed`.
        "#});
}

/// Findings are listed by file, each file once, in the order its first finding
/// came back; a quote or comment spread over lines is put on one.
#[test]
fn the_report_lists_the_findings_by_file() {
    let findings = [
        finding(
            "changelog.d/0.3.0/retry-idle-turns.md",
            "commit_message",
            "Refactor the retry loop.",
            "Say what users notice: a turn that stalls while idle is retried instead of failing.",
        ),
        finding(
            "changelog.d/0.3.0/plugin-manifest-routing.md",
            "no_upgrade_steps",
            "The old lookup\n  is gone.",
            "Say how to rebuild\nor reinstall a plugin.",
        ),
        finding(
            "changelog.d/0.3.0/retry-idle-turns.md",
            "typo",
            "retrys",
            "Write `retries`.",
        ),
        finding(
            "changelog.d/0.3.0/overview.md",
            "no_user_impact",
            "Various improvements.",
            "Name the changes users notice.",
        ),
    ];

    assert_eq!(
        report(&findings, "the notes and upgrade guide of 0.3.0"),
        indoc! {r#"
            changelog.d/0.3.0/retry-idle-turns.md
            - Reads like a commit message: "Refactor the retry loop."
              Say what users notice: a turn that stalls while idle is retried instead of failing.
            - Typo: "retrys"
              Write `retries`.

            changelog.d/0.3.0/plugin-manifest-routing.md
            - Doesn't say what to do: "The old lookup is gone."
              Say how to rebuild or reinstall a plugin.

            changelog.d/0.3.0/overview.md
            - Doesn't say what changed for users: "Various improvements."
              Name the changes users notice.
        "#}
    );
}

#[test]
fn a_review_that_finds_nothing_says_so() {
    assert_eq!(
        report(&[], "the notes and upgrade guide of 0.3.0"),
        "The assistant found nothing to change in the notes and upgrade guide of 0.3.0.\n"
    );
}

#[test]
fn the_json_report_holds_the_findings_as_they_came_back() {
    let findings = [finding("changelog.d/x.md", "typo", "teh", "Write `the`.")];

    assert_eq!(
        report_json(&findings),
        r#"{"findings":[{"file":"changelog.d/x.md","kind":"typo","quote":"teh","comment":"Write `the`."}]}"#
    );
    assert_eq!(report_json(&[]), r#"{"findings":[]}"#);
}
