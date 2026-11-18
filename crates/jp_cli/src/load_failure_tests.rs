use camino::Utf8PathBuf;

use super::*;

fn failure(name: &str, error: &str) -> LoadFailure {
    LoadFailure {
        name: name.to_owned(),
        path: Utf8PathBuf::from(format!("/ws/.jp/conversations/{name}")),
        error: error.to_owned(),
    }
}

#[test]
fn no_failures_renders_nothing() {
    assert_eq!(render(&[]), None);
}

#[test]
fn one_failure_is_listed_in_the_singular() {
    let report = render(&[failure("jp-c17636257526", "missing metadata.json")]);

    assert_eq!(
        report.as_deref(),
        Some(
            "1 conversation failed to load:\n  jp-c17636257526: missing metadata.json\n    \
             /ws/.jp/conversations/jp-c17636257526"
        )
    );
}

#[test]
fn up_to_three_failures_are_listed() {
    let report = render(&[
        failure("jp-c17636257526", "missing events.json"),
        failure(
            "jp-c17636257527",
            "metadata.json: expected value at line 1 column 1",
        ),
        failure("not-a-valid-id", "invalid directory name"),
    ]);

    assert_eq!(
        report.as_deref(),
        Some(
            "3 conversations failed to load:\n  jp-c17636257526: missing events.json\n    \
             /ws/.jp/conversations/jp-c17636257526\n  jp-c17636257527: metadata.json: expected \
             value at line 1 column 1\n    /ws/.jp/conversations/jp-c17636257527\n  \
             not-a-valid-id: invalid directory name\n    /ws/.jp/conversations/not-a-valid-id"
        )
    );
}

#[test]
fn more_than_three_failures_are_counted() {
    let report = render(&[
        failure("jp-c17636257521", "missing events.json"),
        failure("jp-c17636257522", "missing events.json"),
        failure("jp-c17636257523", "missing events.json"),
        failure("jp-c17636257524", "missing events.json"),
    ]);

    assert_eq!(
        report.as_deref(),
        Some("4 conversations failed to load. Run with `-v` to list them.")
    );
}
