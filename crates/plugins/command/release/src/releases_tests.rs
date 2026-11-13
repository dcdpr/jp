use super::*;

/// 2026-10-01 12:00:00 UTC in seconds since the Unix epoch.
const OCTOBER_1: i64 = 1_790_856_000;
const DAY: i64 = 86_400;

fn version(text: &str) -> Version {
    Version::parse(text).unwrap()
}

fn line(branch: &str, started: &[&str]) -> Line {
    Line {
        branch: branch.to_owned(),
        tip: "f".repeat(40),
        started: started
            .iter()
            .zip(0..)
            .map(|(text, day)| Started {
                version: version(text),
                hash: format!("{day:040}"),
                at: OCTOBER_1 + day * DAY,
            })
            .collect(),
        picked: BTreeSet::new(),
    }
}

fn releases(tagged: &[&str], lines: Vec<Line>) -> Releases {
    let tagged: BTreeSet<Version> = tagged.iter().map(|text| version(text)).collect();

    Releases {
        published: tagged.clone(),
        tagged,
        lines: lines
            .into_iter()
            .map(|line| (line_of(&line.branch).unwrap(), line))
            .collect(),
    }
}

#[test]
fn a_release_branch_is_named_after_its_major_and_minor_version() {
    assert_eq!(branch(&version("0.3.1")), "release/0.3");
    assert_eq!(notes_branch(&version("0.3.1")), "release-notes/v0.3.1");
    assert_eq!(
        abandon_branch(&version("0.3.1")),
        "release-notes/abandon-v0.3.1"
    );
    assert_eq!(line_of("release/0.3"), Some((0, 3)));
    assert_eq!(line_of("release/12.0"), Some((12, 0)));

    for name in [
        "release/0.3.1",
        "release/03.1",
        "release/0",
        "release/x.y",
        "release-notes/v0.3.0",
        "main",
    ] {
        assert_eq!(line_of(name), None, "{name}");
    }
}

#[test]
fn a_releases_notes_branch_and_tag_are_named_after_its_version() {
    assert_eq!(notes_branch(&version("0.3.1")), "release-notes/v0.3.1");
    assert_eq!(tag(&version("0.3.1")), "v0.3.1");
}

#[test]
fn a_branch_is_unpushed_when_this_repository_has_commits_of_it_origin_lacks() {
    let unpushed: Vec<Standing> = [
        Standing::Local,
        Standing::Remote,
        Standing::Published,
        Standing::Ahead,
        Standing::Behind,
        Standing::Diverged,
    ]
    .into_iter()
    .filter(|standing| standing.unpushed())
    .collect();

    assert_eq!(unpushed, [Standing::Local, Standing::Ahead]);
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

    let tags = parse_tags(listing);

    assert_eq!(tags.len(), 5);
    assert_eq!(tags["v0.1.0"], "aaaa");
    assert_eq!(
        versions(tags.keys()),
        BTreeSet::from([version("0.1.0"), version("0.2.0")])
    );
}

/// `git ls-remote --tags` lists an annotated tag peeled after its tag object,
/// and the commit is what a tag names.
#[test]
fn an_annotated_tag_names_the_commit_it_peels_to() {
    let listing = concat!(
        "aaaa\trefs/tags/v0.1.0\n",
        "1111\trefs/tags/v0.1.0^{}\n",
        "bbbb\trefs/tags/v0.2.0\n",
    );

    let tags = parse_tags(listing);

    assert_eq!(
        tags,
        BTreeMap::from([
            ("v0.1.0".to_owned(), "1111".to_owned()),
            ("v0.2.0".to_owned(), "bbbb".to_owned()),
        ])
    );
}

#[test]
fn a_release_branch_says_what_it_started_and_what_was_picked_onto_it() {
    // As `git log -z --format=%H%n%ct%n%B` writes it, newest first.
    let log = concat!(
        "3333\n1792843200\nFix the help text\n\nSigned-off-by: Jane Doe <jane@example.com>\n",
        "(cherry picked from commit 9999)\n\0",
        "2222\n1792756800\nStart release 0.3.1\n\nStarts-Release: 0.3.1\n\0",
        "1111\n1792670400\nRetry idle turns\n\n(cherry picked from commit 8888)\n\0",
        "0000\n1792584000\nStart release 0.3.0\n\nStarts-Release: 0.3.0\n",
    );

    let (started, picked) = parse_line(log).unwrap();

    assert_eq!(started, [
        Started {
            version: version("0.3.0"),
            hash: "0000".to_owned(),
            at: 1_792_584_000,
        },
        Started {
            version: version("0.3.1"),
            hash: "2222".to_owned(),
            at: 1_792_756_800,
        },
    ]);
    assert_eq!(
        picked,
        BTreeSet::from(["8888".to_owned(), "9999".to_owned()])
    );
}

/// Only the last paragraph holds trailers, so a message that mentions one
/// earlier doesn't start a release.
#[test]
fn only_the_last_paragraph_holds_trailers() {
    let log = "1111\n1792584000\nDocument the pipeline\n\nStarts-Release: 0.9.0 marks a \
               start.\n\nSee the RFD.\n";

    assert_eq!(parse_line(log), Ok((vec![], BTreeSet::new())));
}

#[test]
fn a_commit_without_a_date_does_not_read_as_a_log() {
    assert_eq!(
        parse_line("1111\nStart release 0.3.0\n\nStarts-Release: 0.3.0\n"),
        Err("The log of a release branch gives commit 1111 no date.".to_owned())
    );
}

#[test]
fn the_start_message_carries_the_trailer_it_is_read_back_by() {
    let message = start_message(&version("0.3.0"));

    assert_eq!(
        message,
        "chore(release): Start release 0.3.0\n\nStarts-Release: 0.3.0\n"
    );
    assert_eq!(
        parse_line(&format!("0000\n{OCTOBER_1}\n{message}"))
            .unwrap()
            .0,
        [Started {
            version: version("0.3.0"),
            hash: "0000".to_owned(),
            at: OCTOBER_1,
        }]
    );
}

#[test]
fn a_tags_time_is_read_from_its_tagger_line() {
    let tag = "object 4e1f0a2c9d8b7a6f5e4d3c2b1a0f9e8d7c6b5a49\ntype commit\ntag v0.1.0\ntagger \
               Jane Doe <jane@example.com> 1792584000 +0200\n\nRelease 0.1.0\ntagger in the \
               message 1 +0000\n";

    assert_eq!(tagger_time(tag), Some(1_792_584_000));
    // A tag made without a tagger, as some old tools did.
    assert_eq!(
        tagger_time("object 4e1f\ntype commit\ntag v0.1.0\n\nRelease 0.1.0\n"),
        None
    );
}

/// The newest minor or major release is the last one cut, whether a tag or a
/// release branch says so, and a patch release never is one.
#[test]
fn the_newest_cut_is_the_newest_minor_or_major_release() {
    assert_eq!(Releases::default().newest_cut(), None);
    assert_eq!(
        releases(&["0.1.0", "0.2.0", "0.2.1"], vec![]).newest_cut(),
        Some(&version("0.2.0"))
    );
    assert_eq!(
        releases(&["0.2.0"], vec![
            line("release/0.2", &["0.2.0", "0.2.1"]),
            line("release/0.3", &["0.3.0"]),
        ])
        .newest_cut(),
        Some(&version("0.3.0"))
    );
}

#[test]
fn a_release_was_started_by_the_commit_on_its_branch_that_starts_it() {
    let releases = releases(&["0.3.0"], vec![line("release/0.3", &["0.3.0", "0.3.1"])]);

    assert_eq!(releases.started_at(&version("0.3.0")), Some(OCTOBER_1));
    assert_eq!(
        releases.started_at(&version("0.3.1")),
        Some(OCTOBER_1 + DAY)
    );
    // Tagged by hand, or not started at all.
    assert_eq!(releases.started_at(&version("0.2.0")), None);
    assert_eq!(releases.started_at(&version("0.3.2")), None);
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
        published: BTreeSet::new(),
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

/// A tag only this repository has finalizes the release once it is pushed.
#[test]
fn finalize_points_at_a_tag_made_here_to_publish() {
    let mut releases = releases(&["0.3.0"], vec![line("release/0.3", &["0.3.0", "0.3.1"])]);
    releases.tagged.insert(version("0.3.1"));

    assert_eq!(
        releases.finalize_target(&version("0.3.1")),
        Err(
            "0.3.1 is tagged in this repository already; publish the tag with `git push origin \
             v0.3.1`."
                .to_owned()
        )
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
