use super::*;

/// A commit changing `changes`, with a hash made of `id`.
fn commit(id: char, changes: Vec<Change>) -> Commit {
    Commit {
        hash: id.to_string().repeat(40),
        message: format!("Commit {id}\n"),
        changes,
    }
}

fn added(path: &str) -> Change {
    Change::Added(path.to_owned())
}

fn modified(path: &str) -> Change {
    Change::Modified(path.to_owned())
}

fn renamed(from: &str, to: &str) -> Change {
    Change::Renamed {
        from: from.to_owned(),
        to: to.to_owned(),
    }
}

#[test]
fn the_log_is_read_into_commits_and_their_changes() {
    // As git writes it: newest first, a NUL after each header and each field,
    // and a line break between a header and its changes.
    let log = concat!(
        "commit 8089f0f2e1d88c54790bca537e01c83e213586da\nCut 0.1.0 (#4)\n\0",
        "\nR100\0a.md\0",
        "0.1.0/a.md\0",
        "commit cee4e79d628f4f28a64e59e7640939b04ae201b1\nMerge pull request #3 from x/feature\n\0",
        "\nA\0b.md\0M\0highlights/fan-out.md\0",
        "commit 0cafc5d22dd31325294b60891968f81021c5cc5a\nAdd a (#1)\n\nWith a body.\n\0",
        "\nA\0a.md\0",
    );

    assert_eq!(parse(log).unwrap(), vec![
        Commit {
            hash: "8089f0f2e1d88c54790bca537e01c83e213586da".to_owned(),
            message: "Cut 0.1.0 (#4)\n".to_owned(),
            changes: vec![renamed("a.md", "0.1.0/a.md")],
        },
        Commit {
            hash: "cee4e79d628f4f28a64e59e7640939b04ae201b1".to_owned(),
            message: "Merge pull request #3 from x/feature\n".to_owned(),
            changes: vec![added("b.md"), modified("highlights/fan-out.md")],
        },
        Commit {
            hash: "0cafc5d22dd31325294b60891968f81021c5cc5a".to_owned(),
            message: "Add a (#1)\n\nWith a body.\n".to_owned(),
            changes: vec![added("a.md")],
        },
    ]);
}

#[test]
fn an_empty_log_has_no_commits() {
    assert_eq!(parse(""), Ok(vec![]));
}

/// A path is read as written, whatever it holds, since it can't be mistaken for
/// a header where a path is due.
#[test]
fn paths_are_read_as_written() {
    let log = "commit aaaa\nAdd\n\0\nA\0commit notes.md\0D\0old.md\0C75\0a.md\0copy.md\0";

    assert_eq!(parse(log).unwrap()[0].changes, vec![
        added("commit notes.md"),
        Change::Deleted("old.md".to_owned()),
        added("copy.md"),
    ]);
}

#[test]
fn a_log_that_does_not_read_as_one_is_an_error() {
    assert_eq!(
        parse("\nA\0a.md\0"),
        Err("the log lists `A` before any commit".to_owned())
    );
    assert_eq!(
        parse("commit aaaa\nAdd\n\0\nR100\0a.md\0"),
        Err("the log names no path after `R100`".to_owned())
    );
    assert_eq!(
        parse("commit aaaa\nAdd\n\0\nX\0a.md\0"),
        Err("the log lists an unknown change, `X`".to_owned())
    );
}

#[test]
fn every_commit_that_changed_a_file_at_the_top_level_is_credited_oldest_first() {
    let commits = [
        commit('c', vec![modified("a.md")]),
        commit('b', vec![added("b.md"), modified("a.md")]),
        commit('a', vec![added("a.md")]),
    ];

    assert_eq!(credited(&commits, &["a.md", "b.md"]), vec![
        vec![2, 1, 0],
        vec![1]
    ]);
}

#[test]
fn a_released_file_is_credited_up_to_the_commit_that_filed_it() {
    let commits = [
        // A correction to the released copy.
        commit('d', vec![modified("0.3.0/a.md")]),
        // The cut.
        commit('c', vec![renamed("a.md", "0.3.0/a.md")]),
        commit('b', vec![modified("a.md")]),
        commit('a', vec![added("a.md")]),
    ];

    assert_eq!(credited(&commits, &["0.3.0/a.md"]), vec![vec![3, 2]]);
}

#[test]
fn a_highlight_is_followed_the_same_way() {
    let commits = [
        commit('c', vec![renamed(
            "highlights/fan-out.md",
            "0.3.0/highlights/fan-out.md",
        )]),
        commit('b', vec![modified("highlights/fan-out.md")]),
        commit('a', vec![added("highlights/fan-out.md")]),
    ];

    assert_eq!(credited(&commits, &["0.3.0/highlights/fan-out.md"]), vec![
        vec![2, 1]
    ]);
}

#[test]
fn a_rename_within_the_top_level_credits_and_is_followed() {
    let commits = [
        commit('b', vec![renamed("retry.md", "retry-idle-turns.md")]),
        commit('a', vec![added("retry.md")]),
    ];

    assert_eq!(credited(&commits, &["retry-idle-turns.md"]), vec![vec![
        1, 0
    ]]);
}

/// Moving a file back out of a release, as abandoning one does, writes nothing.
#[test]
fn moving_a_file_out_of_a_release_credits_nobody() {
    let commits = [
        commit('c', vec![renamed("0.3.0/a.md", "a.md")]),
        commit('b', vec![renamed("a.md", "0.3.0/a.md")]),
        commit('a', vec![added("a.md")]),
    ];

    assert_eq!(credited(&commits, &["a.md"]), vec![vec![2]]);
}

/// A copy filed into a patch release starts its own history there, and is
/// credited through its frontmatter instead.
#[test]
fn a_file_added_in_a_release_is_credited_to_nobody() {
    let commits = [commit('a', vec![added("0.3.1/a.md")])];

    assert_eq!(credited(&commits, &["0.3.1/a.md"]), vec![
        Vec::<usize>::new()
    ]);
}

/// A file written again where an older one was deleted has a history of its
/// own.
#[test]
fn history_ends_where_the_file_was_added() {
    let commits = [
        commit('c', vec![added("a.md")]),
        commit('b', vec![Change::Deleted("a.md".to_owned())]),
        commit('a', vec![added("a.md")]),
    ];

    assert_eq!(credited(&commits, &["a.md"]), vec![vec![0]]);
}

/// One commit can free a path and take it for another file: each change is read
/// against where the files were after the commit.
#[test]
fn a_path_freed_and_taken_in_one_commit_is_followed_to_the_right_file() {
    let commits = [
        commit('b', vec![renamed("a.md", "b.md"), added("a.md")]),
        commit('a', vec![added("a.md")]),
    ];

    assert_eq!(credited(&commits, &["a.md", "b.md"]), vec![vec![0], vec![
        1, 0
    ]]);
}

#[test]
fn a_file_history_does_not_know_is_credited_to_nobody() {
    let commits = [commit('a', vec![added("a.md")])];

    assert_eq!(credited(&commits, &["new.md"]), vec![Vec::<usize>::new()]);
}
