use super::*;

fn version(text: &str) -> Version {
    Version::parse(text).unwrap()
}

/// A release branch holding the releases `started`, oldest first, with nothing
/// picked onto it.
fn line(branch: &str, started: &[&str]) -> Line {
    Line {
        branch: branch.to_owned(),
        tip: "f".repeat(40),
        started: started.iter().map(|text| version(text)).collect(),
        picked: BTreeSet::new(),
    }
}

/// The releases `tagged` finalized, and `lines` holds.
fn releases(tagged: &[&str], lines: Vec<Line>) -> Releases {
    Releases {
        tagged: tagged.iter().map(|text| version(text)).collect(),
        lines: lines
            .into_iter()
            .map(|line| (line_of(&line.branch).unwrap(), line))
            .collect(),
    }
}

#[test]
fn a_release_branch_is_named_after_its_major_and_minor_version() {
    assert_eq!(branch(&version("0.3.1")), "release/0.3");
    assert_eq!(line_of("release/0.3"), Some((0, 3)));
    assert_eq!(line_of("release/12.0"), Some((12, 0)));

    for name in [
        "release/0.3.1",
        "release/03.1",
        "release/0",
        "release/x.y",
        "main",
    ] {
        assert_eq!(line_of(name), None, "{name}");
    }
}

#[test]
fn tags_name_a_v_and_a_version_that_is_no_pre_release() {
    let listing = concat!(
        "aaaa\trefs/tags/v0.1.0\n",
        "bbbb\trefs/tags/v0.2.0\n",
        "cccc\trefs/tags/v1.0.0-rc.1\n",
        "dddd\trefs/tags/nightly\n",
        "eeee\trefs/tags/0.3.0\n",
    );

    assert_eq!(
        parse_tags(listing),
        BTreeSet::from([version("0.1.0"), version("0.2.0")])
    );
}

#[test]
fn a_release_branch_says_what_it_started_and_what_was_picked_onto_it() {
    // As `git log -z --format=%H%n%B` writes it, newest first.
    let log = concat!(
        "3333\nFix the help text\n\nSigned-off-by: Jane Doe <jane@example.com>\n",
        "(cherry picked from commit 9999)\n\0",
        "2222\nStart release 0.3.1\n\nStarts-Release: 0.3.1\n\0",
        "1111\nRetry idle turns\n\n(cherry picked from commit 8888)\n\0",
        "0000\nStart release 0.3.0\n\nStarts-Release: 0.3.0\n",
    );

    let (started, picked) = parse_line(log);

    assert_eq!(started, [version("0.3.0"), version("0.3.1")]);
    assert_eq!(
        picked,
        BTreeSet::from(["8888".to_owned(), "9999".to_owned()])
    );
}

/// Only the last paragraph holds trailers, so a message that mentions one
/// earlier doesn't start a release.
#[test]
fn only_the_last_paragraph_holds_trailers() {
    let log =
        "1111\nDocument the pipeline\n\nStarts-Release: 0.9.0 marks a start.\n\nSee the RFD.\n";

    assert_eq!(parse_line(log), (vec![], BTreeSet::new()));
}

#[test]
fn the_start_message_carries_the_trailer_it_is_read_back_by() {
    let message = start_message(&version("0.3.0"));

    assert_eq!(message, "Start release 0.3.0\n\nStarts-Release: 0.3.0\n");
    assert_eq!(parse_line(&format!("0000\n{message}")).0, [version(
        "0.3.0"
    )]);
}

#[test]
fn only_a_major_release_or_a_minor_one_before_1_0_can_break() {
    assert!(can_break(&version("0.4.0")));
    assert!(can_break(&version("2.0.0")));

    assert!(!can_break(&version("0.3.1")));
    assert!(!can_break(&version("1.3.0")));
}

#[test]
fn the_first_release_is_0_1_0() {
    assert_eq!(
        Releases::default().next_cut(true, None),
        Ok(version("0.1.0"))
    );
}

/// JP tagged v0.1.0 by hand, without a release branch.
#[test]
fn a_cut_after_a_tag_is_the_next_minor_release() {
    let releases = releases(&["0.1.0"], vec![]);

    assert_eq!(releases.next_cut(false, None), Ok(version("0.2.0")));
    // Before 1.0, a breaking change makes a minor release too.
    assert_eq!(releases.next_cut(true, None), Ok(version("0.2.0")));
}

/// A cut while another release is in preparation cuts the one after it.
#[test]
fn a_cut_follows_the_releases_in_preparation() {
    let releases = releases(&["0.2.0"], vec![
        line("release/0.2", &["0.2.0"]),
        line("release/0.3", &["0.3.0"]),
    ]);

    assert_eq!(releases.next_cut(false, None), Ok(version("0.4.0")));
}

#[test]
fn from_1_0_on_a_breaking_change_cuts_a_major_release() {
    let releases = releases(&["1.2.0"], vec![line("release/1.2", &["1.2.0"])]);

    assert_eq!(releases.next_cut(false, None), Ok(version("1.3.0")));
    assert_eq!(releases.next_cut(true, None), Ok(version("2.0.0")));
}

#[test]
fn a_requested_version_overrides_the_next_one() {
    let releases = releases(&["0.9.0"], vec![]);

    assert_eq!(
        releases.next_cut(false, Some(&version("1.0.0"))),
        Ok(version("1.0.0"))
    );
}

#[test]
fn a_requested_version_the_cut_cannot_start_is_refused() {
    let releases = releases(&["0.3.0"], vec![line("release/0.3", &["0.3.0"])]);

    assert_eq!(
        releases.next_cut(false, Some(&version("0.3.1"))),
        Err(
            "0.3.1 is a patch release, which `jp release pick 0.3.1 <commit>...` starts on its \
             release branch; a cut starts a minor or major release."
                .to_owned()
        )
    );
    assert_eq!(
        releases.next_cut(false, Some(&version("0.2.0"))),
        Err("0.2.0 isn't past 0.3.0, the newest release tagged or started.".to_owned())
    );
    assert_eq!(
        releases.next_cut(false, Some(&version("0.4.0-rc.1"))),
        Err("0.4.0-rc.1 is a pre-release, and `jp release` makes none.".to_owned())
    );
}

/// A release branch abandoned without its version ever being tagged still holds
/// its line's name.
#[test]
fn a_cut_onto_an_existing_release_branch_is_refused() {
    let releases = Releases {
        tagged: BTreeSet::new(),
        lines: BTreeMap::from([((0, 4), line("release/0.4", &[]))]),
    };

    assert_eq!(
        releases.next_cut(false, Some(&version("0.4.0"))),
        Err("release/0.4 exists already.".to_owned())
    );
}

#[test]
fn a_pick_goes_into_the_release_in_preparation() {
    let releases = releases(&[], vec![line("release/0.3", &["0.3.0"])]);

    let target = releases.pick_target(&version("0.3.0")).unwrap();

    assert_eq!(target.line.branch, "release/0.3");
    assert!(!target.start);
}

#[test]
fn a_pick_after_the_last_release_was_tagged_starts_the_next_patch_release() {
    let releases = releases(&["0.3.0", "0.3.1"], vec![line("release/0.3", &[
        "0.3.0", "0.3.1",
    ])]);

    let target = releases.pick_target(&version("0.3.2")).unwrap();

    assert!(target.start);
    assert_eq!(
        releases.pick_target(&version("0.3.3")),
        Err("The next release on release/0.3 is 0.3.2, so pick into that.".to_owned())
    );
}

#[test]
fn a_pick_waits_for_the_release_in_preparation_to_be_tagged() {
    let releases = releases(&["0.3.0"], vec![line("release/0.3", &["0.3.0", "0.3.1"])]);

    assert_eq!(
        releases.pick_target(&version("0.3.2")),
        Err(
            "0.3.1 is in preparation on release/0.3; pick into it, or finalize it before starting \
             another release there."
                .to_owned()
        )
    );
}

#[test]
fn a_pick_needs_a_release_branch() {
    let releases = releases(&["0.3.0"], vec![]);

    assert_eq!(
        releases.pick_target(&version("0.4.0")),
        Err("0.4.0 hasn't been cut; cut it with `jp release cut --version 0.4.0`.".to_owned())
    );
    assert_eq!(
        releases.pick_target(&version("0.3.1")),
        Err("There is no release/0.3, so 0.3.1 has no release branch to pick onto.".to_owned())
    );
}

#[test]
fn finalize_tags_the_release_in_preparation() {
    let releases = releases(&["0.3.0"], vec![line("release/0.3", &["0.3.0", "0.3.1"])]);

    assert_eq!(
        releases
            .finalize_target(&version("0.3.1"))
            .map(|line| &line.branch),
        Ok(&"release/0.3".to_owned())
    );
    assert_eq!(
        releases.finalize_target(&version("0.3.0")),
        Err("0.3.0 is finalized already: origin has its tag v0.3.0.".to_owned())
    );
    assert_eq!(
        releases.finalize_target(&version("0.3.2")),
        Err("0.3.1, not 0.3.2, is in preparation on release/0.3.".to_owned())
    );
}

#[test]
fn finalize_refuses_a_release_that_has_not_started() {
    let releases = releases(&["0.3.0"], vec![line("release/0.3", &["0.3.0"])]);

    assert_eq!(
        releases.finalize_target(&version("0.3.1")),
        Err(
            "0.3.1 isn't in preparation on release/0.3; pick a commit into it with `jp release \
             pick 0.3.1 <commit>` to start it."
                .to_owned()
        )
    );
    assert_eq!(
        releases.finalize_target(&version("0.4.0")),
        Err("There is no release/0.4, so 0.4.0 hasn't been cut.".to_owned())
    );
}

#[test]
fn a_change_ships_with_the_branches_holding_it_or_picked_onto() {
    let mut picked_onto = line("release/0.2", &["0.2.0"]);
    picked_onto.picked.insert("aaaa".to_owned());
    let releases = releases(&[], vec![
        picked_onto,
        line("release/0.3", &["0.3.0"]),
        line("release/0.4", &["0.4.0"]),
    ]);

    let shipping: Vec<&str> = releases
        .shipping("aaaa", &["release/0.4".to_owned()])
        .into_iter()
        .map(|line| line.branch.as_str())
        .collect();

    assert_eq!(shipping, ["release/0.2", "release/0.4"]);
    assert_eq!(releases.shipping("bbbb", &[]), Vec::<&Line>::new());
}
