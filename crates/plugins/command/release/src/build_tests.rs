use camino_tempfile::Utf8TempDir;
use serde_json::{Value, json};

use super::*;
use crate::{cut::cut, finalize::finalize, options::Options, repository_testing::Fixture};

/// A `bump` that writes the version into `VERSION`, wherever git runs.
const BUMP: &str = "git config --file VERSION version.current {version}";

/// The command the tests build with: `build.sh`, which a test writes into the
/// workspace root.
const BUILD: &str = "sh build.sh {version} {source} {out} {profile}";

/// A commit, for checks that build nothing.
const COMMIT: &str = "8a9b0c1d2e3f4a5b6c7d8e9f0a1b2c3d4e5f6a7b";

/// The archive the tests' builds write.
const ARCHIVE: &str = "jp-x86_64-unknown-linux-gnu.tar.xz";

/// What [`ARCHIVE`] holds.
const CONTENTS: &str = "an archive\n";

/// The SHA-256 of [`CONTENTS`], as `shasum -a 256` gives it.
const HASH: &str = "4cd36b3ca046f7c17e81e1aa31bf2dc6a91b98c94238810db57456b4904d3a22";

/// The platform [`ARCHIVE`] is for.
const TARGET: &str = "x86_64-unknown-linux-gnu";

fn options_with(options: Value) -> Options {
    let Value::Object(options) = options else {
        panic!("options are an object");
    };

    Options::read(&options).unwrap()
}

fn options() -> Options {
    options_with(json!({ "forge": "none", "bump": BUMP, "build": BUILD }))
}

fn version(text: &str) -> Version {
    Version::parse(text).unwrap()
}

/// Run `command` in the workspace of `fixture`.
fn run(
    fixture: &Fixture,
    command: impl FnOnce(&Pipeline<'_>) -> Result<String, String>,
) -> Result<String, String> {
    let shell = fixture.shell();
    let options = options();
    let pipeline = Pipeline::new(&shell, fixture.root(), &options).unwrap();

    command(&pipeline)
}

/// Build `version` with `profile` in the workspace of `fixture`, keeping it in
/// `builds`: what `build` returned, and every line it handed on.
fn build_in(
    fixture: &Fixture,
    builds: &Utf8Path,
    version: &str,
    profile: Profile,
) -> (Result<String, String>, Vec<String>) {
    let version = Version::parse(version).unwrap();
    let mut lines = vec![];
    let built = run(fixture, |pipeline| {
        build(pipeline, builds, &version, profile, &mut |line| {
            lines.push(line.to_owned());
            Ok(())
        })
    });

    (built, lines)
}

/// A workspace whose release 0.1.0, listing one fix, was cut here.
fn cut_0_1_0() -> Fixture {
    let fixture = Fixture::new();
    fixture.fragment("retry", "fixed", "Retried.");
    fixture.commit("fix: Retry (#1)");
    fixture.push();
    run(&fixture, |pipeline| cut(pipeline, None, None)).unwrap();

    fixture
}

/// [`cut_0_1_0`], with its notes merged, finalized, and everything published.
fn finalized_0_1_0() -> Fixture {
    let fixture = cut_0_1_0();
    fixture.publish(&["release/0.1", "release-notes/v0.1.0"]);
    fixture.merge_notes("0.1.0", "chore(release): Cut release 0.1.0");
    run(&fixture, |pipeline| finalize(pipeline, &version("0.1.0"))).unwrap();
    fixture.publish(&["v0.1.0"]);

    fixture
}

/// A `manifest.json` naming [`ARCHIVE`] for [`TARGET`].
fn manifest(version: &str, commit: &str, profile: &str) -> String {
    json!({
        "format": 1,
        "version": version,
        "commit": commit,
        "profile": profile,
        "artifacts": [{ "file": ARCHIVE, "target": TARGET }],
    })
    .to_string()
}

/// A directory holding what a build of 0.3.0 from [`COMMIT`] with the release
/// profile writes.
fn written() -> Utf8TempDir {
    let out = Utf8TempDir::new().unwrap();
    fs::write(out.path().join(ARCHIVE), CONTENTS).unwrap();
    fs::write(
        out.path().join("checksums.txt"),
        format!("{HASH}  {ARCHIVE}\n"),
    )
    .unwrap();
    fs::write(
        out.path().join("manifest.json"),
        manifest("0.3.0", COMMIT, "release"),
    )
    .unwrap();

    out
}

/// The names in `dir`, sorted.
fn names(dir: &Utf8Path) -> Vec<String> {
    let mut names: Vec<String> = dir
        .read_dir_utf8()
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_owned())
        .collect();
    names.sort();

    names
}

#[test]
fn files_are_hashed_with_sha256_in_lowercase_hex() {
    let dir = Utf8TempDir::new().unwrap();
    let empty = dir.path().join("empty");
    let abc = dir.path().join("abc");
    fs::write(&empty, "").unwrap();
    fs::write(&abc, "abc").unwrap();

    assert_eq!(
        sha256(&empty),
        Ok("e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855".to_owned())
    );
    assert_eq!(
        sha256(&abc),
        Ok("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad".to_owned())
    );
}

#[test]
fn a_build_that_wrote_what_was_asked_for_is_accepted() {
    let out = written();

    let manifest = check(out.path(), &version("0.3.0"), COMMIT, Profile::Release).unwrap();

    assert_eq!(manifest.artifacts, [Artifact {
        file: ARCHIVE.to_owned(),
        target: TARGET.to_owned(),
    }]);
}

/// `sha256sum` marks a file it read in binary mode with `*`, and a hash may be
/// written in either case.
#[test]
fn checksums_are_read_as_sha256sum_writes_them() {
    let upper = HASH.to_ascii_uppercase();

    assert_eq!(
        parse_checksum(&format!("{HASH}  {ARCHIVE}")),
        Some((HASH.to_owned(), ARCHIVE))
    );
    assert_eq!(
        parse_checksum(&format!("{upper} *{ARCHIVE}")),
        Some((HASH.to_owned(), ARCHIVE))
    );
    assert_eq!(parse_checksum(&format!("{HASH} {ARCHIVE}")), None);
    assert_eq!(parse_checksum(&format!("{HASH}  ")), None);
    assert_eq!(
        parse_checksum(&format!("SHA256 ({ARCHIVE}) = {HASH}")),
        None
    );
}

/// Every problem is listed at once, so a build command can be fixed in one go.
#[test]
fn everything_wrong_with_what_a_build_wrote_is_listed() {
    let out = written();
    let dir = out.path();
    fs::write(dir.join("jp-linux-copy.tar.xz"), CONTENTS).unwrap();
    fs::write(dir.join("build.log"), "compiled\n").unwrap();
    fs::write(
        dir.join("checksums.txt"),
        format!(
            "{zeros}  {ARCHIVE}\nnot a checksum\n{HASH}  stray.tar.xz\n",
            zeros = "0".repeat(64)
        ),
    )
    .unwrap();
    fs::write(
        dir.join("manifest.json"),
        json!({
            "format": 1,
            "version": "0.3.1",
            "commit": "8a9b0c1",
            "profile": "quick",
            "artifacts": [
                { "file": ARCHIVE, "target": TARGET },
                { "file": "../escape.tar.xz", "target": "aarch64-apple-darwin" },
                { "file": ARCHIVE, "target": "x86_64-apple-darwin" },
                { "file": "jp-mac.tar.xz", "target": " " },
                { "file": "jp-linux-copy.tar.xz", "target": TARGET },
            ],
        })
        .to_string(),
    )
    .unwrap();

    let problems = check(dir, &version("0.3.0"), COMMIT, Profile::Release).unwrap_err();

    assert_eq!(problems, [
        "manifest.json names version 0.3.1, but the build was of 0.3.0.",
        "manifest.json names commit `8a9b0c1`, but the build was of \
         8a9b0c1d2e3f4a5b6c7d8e9f0a1b2c3d4e5f6a7b.",
        "manifest.json names the quick profile, but the build was asked for the release profile.",
        "manifest.json lists `../escape.tar.xz`, which can't be the name of an archive in the \
         build's directory.",
        "manifest.json lists `jp-x86_64-unknown-linux-gnu.tar.xz` twice.",
        "manifest.json names no target for `jp-mac.tar.xz`.",
        "manifest.json lists `jp-mac.tar.xz`, which the build didn't write.",
        "manifest.json lists two archives for x86_64-unknown-linux-gnu: \
         `jp-x86_64-unknown-linux-gnu.tar.xz` and `jp-linux-copy.tar.xz`.",
        "checksums.txt line 2 isn't a SHA-256 and a file name, as `sha256sum` writes them.",
        "checksums.txt lists `stray.tar.xz`, which manifest.json doesn't.",
        "checksums.txt has no SHA-256 for `jp-linux-copy.tar.xz`.",
        "checksums.txt has no SHA-256 for `jp-mac.tar.xz`.",
        "checksums.txt gives `jp-x86_64-unknown-linux-gnu.tar.xz` the SHA-256 \
         0000000000000000000000000000000000000000000000000000000000000000, but its contents hash \
         to 4cd36b3ca046f7c17e81e1aa31bf2dc6a91b98c94238810db57456b4904d3a22.",
        "The build left `build.log`, which manifest.json doesn't list.",
    ]);
}

#[test]
fn a_manifest_that_cannot_be_read_stops_the_check() {
    let check_with = |manifest: Option<&str>| {
        let out = written();
        match manifest {
            Some(manifest) => fs::write(out.path().join("manifest.json"), manifest).unwrap(),
            None => fs::remove_file(out.path().join("manifest.json")).unwrap(),
        }

        check(out.path(), &version("0.3.0"), COMMIT, Profile::Release).unwrap_err()
    };

    assert_eq!(check_with(None), ["The build wrote no manifest.json."]);
    assert_eq!(check_with(Some(r#"{"format": 2, "builds": []}"#)), [
        "manifest.json is format 2, and this jp-release reads format 1."
    ]);
    assert_eq!(check_with(Some(r#"{"version": "0.3.0"}"#)), [
        "manifest.json names no format."
    ]);
    let unreadable = check_with(Some(r#"{"format": 1, "version": "0.3.0"}"#));
    assert!(
        unreadable[0].starts_with("manifest.json can't be read: missing field `commit`"),
        "{unreadable:?}"
    );
    let not_json = check_with(Some("version = \"0.3.0\""));
    assert!(
        not_json[0].starts_with("manifest.json isn't JSON: "),
        "{not_json:?}"
    );
}

#[test]
fn a_kept_build_is_publishable_when_made_with_the_release_profile_from_the_tagged_commit() {
    let builds = Utf8TempDir::new().unwrap();
    let assess_with = |manifest: &str, tagged: &str| {
        let dir = builds.path().join("0.3.0");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("manifest.json"), manifest).unwrap();

        assess(builds.path(), &version("0.3.0"), tagged)
    };

    assert_eq!(
        assess(builds.path(), &version("0.3.0"), COMMIT),
        Kept::Nothing
    );
    assert_eq!(
        assess_with(&manifest("0.3.0", COMMIT, "release"), COMMIT),
        Kept::Publishable
    );
    assert_eq!(
        assess_with(&manifest("0.3.0", COMMIT, "quick"), COMMIT),
        Kept::ForTesting("a quick build".to_owned())
    );
    assert_eq!(
        assess_with(&manifest("0.3.0", COMMIT, "release"), "1111111"),
        Kept::ForTesting("a build of 8a9b0c1, which v0.3.0 doesn't name".to_owned())
    );
    assert_eq!(
        assess_with("{", COMMIT),
        Kept::ForTesting("a build without a manifest.json it can read".to_owned())
    );
}

#[test]
fn without_a_build_command_nothing_is_built() {
    let fixture = Fixture::new();
    let builds = Utf8TempDir::new().unwrap();
    let shell = fixture.shell();
    let options = options_with(json!({ "forge": "none" }));
    let pipeline = Pipeline::new(&shell, fixture.root(), &options).unwrap();

    let error = build(
        &pipeline,
        builds.path(),
        &version("0.1.0"),
        Profile::Release,
        &mut |_| Ok(()),
    )
    .unwrap_err();

    assert_eq!(
        error,
        "`plugins.command.release.options.build` isn't set, so there is no command to build 0.1.0 \
         with."
    );
}

#[test]
fn a_release_neither_tagged_nor_in_preparation_is_not_built() {
    let fixture = cut_0_1_0();
    let builds = Utf8TempDir::new().unwrap();

    let (uncut, _) = build_in(&fixture, builds.path(), "0.9.0", Profile::Release);
    let (unstarted, _) = build_in(&fixture, builds.path(), "0.1.1", Profile::Release);

    assert_eq!(
        uncut,
        Err("There is no release/0.9, so 0.9.0 hasn't been cut.".to_owned())
    );
    assert_eq!(
        unstarted,
        Err(
            "0.1.1 is neither tagged nor in preparation on release/0.1, so there is nothing of it \
             to build."
                .to_owned()
        )
    );
    assert_eq!(names(builds.path()), Vec::<String>::new());
}

/// The builds below run `build.sh` with `sh`.
#[cfg(unix)]
mod built {
    use super::*;

    /// A build script that prints a line on stdout and one on stderr, then
    /// writes [`ARCHIVE`], its checksum, and a manifest naming what it was
    /// asked for, or `commit` for the commit when given.
    fn script(commit: Option<&str>) -> String {
        let commit = commit.map_or_else(
            || r#""$(git -C "$source" rev-parse HEAD)""#.to_owned(),
            ToOwned::to_owned,
        );

        format!(
            r#"set -e
version=$1 source=$2 out=$3 profile=$4
echo "compiling jp $version"
echo "warning: nothing to warn about" >&2
printf 'an archive\n' > "$out/{ARCHIVE}"
printf '%s  %s\n' {HASH} {ARCHIVE} > "$out/checksums.txt"
commit={commit}
printf '{{"format":1,"version":"%s","commit":"%s","profile":"%s","artifacts":[{{"file":"{ARCHIVE}","target":"{TARGET}"}}]}}\n' \
    "$version" "$commit" "$profile" > "$out/manifest.json"
"#
        )
    }

    /// What `manifest.json` of the build of `version` kept in `builds` names.
    fn kept(builds: &Utf8Path, version: &str) -> Manifest {
        read_manifest(&builds.join(version).join("manifest.json")).unwrap()
    }

    #[test]
    fn a_finalized_release_is_built_from_the_commit_its_tag_names_and_kept() {
        let fixture = finalized_0_1_0();
        fixture.write("build.sh", &script(None));
        let builds = Utf8TempDir::new().unwrap();
        let tagged = fixture.hash("v0.1.0^{commit}");

        let (built, lines) = build_in(&fixture, builds.path(), "0.1.0", Profile::Release);

        assert_eq!(
            built.unwrap(),
            format!(
                "Built 0.1.0 from v0.1.0 at {} with the release profile, into {}:\n\n    \
                 jp-x86_64-unknown-linux-gnu.tar.xz  x86_64-unknown-linux-gnu\n",
                &tagged[..7],
                builds.path().join("0.1.0")
            )
        );
        assert_eq!(
            lines[0],
            format!(
                "Building 0.1.0 from v0.1.0 at {} with the release profile.",
                &tagged[..7]
            )
        );
        let mut printed = lines[1..].to_vec();
        printed.sort();
        assert_eq!(printed, [
            "compiling jp 0.1.0",
            "warning: nothing to warn about"
        ]);
        assert_eq!(names(builds.path()), ["0.1.0"]);
        assert_eq!(names(&builds.path().join("0.1.0")), [
            "checksums.txt",
            "jp-x86_64-unknown-linux-gnu.tar.xz",
            "manifest.json"
        ]);
        assert_eq!(kept(builds.path(), "0.1.0").commit, tagged);
        assert_eq!(
            assess(builds.path(), &version("0.1.0"), &tagged),
            Kept::Publishable
        );
    }

    #[test]
    fn a_release_in_preparation_is_built_from_its_branch_for_testing() {
        let fixture = cut_0_1_0();
        fixture.write("build.sh", &script(None));
        let builds = Utf8TempDir::new().unwrap();
        let tip = fixture.hash("release/0.1");

        let (built, _) = build_in(&fixture, builds.path(), "0.1.0", Profile::Release);

        assert_eq!(
            built.unwrap(),
            format!(
                "Built 0.1.0 from release/0.1 at {short} with the release profile, into {}:\n\n    \
                 jp-x86_64-unknown-linux-gnu.tar.xz  x86_64-unknown-linux-gnu\n\n0.1.0 isn't \
                 tagged yet, so publishers refuse this build until v0.1.0 names {short}.\n",
                builds.path().join("0.1.0"),
                short = &tip[..7]
            )
        );
        assert_eq!(kept(builds.path(), "0.1.0").commit, tip);
    }

    #[test]
    fn a_quick_build_replaces_an_earlier_build_for_testing() {
        let fixture = finalized_0_1_0();
        fixture.write("build.sh", &script(None));
        let builds = Utf8TempDir::new().unwrap();
        let tagged = fixture.hash("v0.1.0^{commit}");
        build_in(&fixture, builds.path(), "0.1.0", Profile::Release)
            .0
            .unwrap();

        let (built, lines) = build_in(&fixture, builds.path(), "0.1.0", Profile::Quick);

        assert_eq!(
            built.unwrap(),
            format!(
                "Built 0.1.0 from v0.1.0 at {} with the quick profile, into {}:\n\n    \
                 jp-x86_64-unknown-linux-gnu.tar.xz  x86_64-unknown-linux-gnu\n\nIt replaces the \
                 build of 0.1.0 kept there before.\n\nIt's a quick build, for testing: publishers \
                 refuse it.\n",
                &tagged[..7],
                builds.path().join("0.1.0")
            )
        );
        assert!(lines[0].ends_with(" with the quick profile."), "{lines:?}");
        assert_eq!(names(builds.path()), ["0.1.0"]);
        assert_eq!(
            assess(builds.path(), &version("0.1.0"), &tagged),
            Kept::ForTesting("a quick build".to_owned())
        );
    }

    #[test]
    fn a_failed_build_keeps_nothing() {
        let fixture = finalized_0_1_0();
        fixture.write("build.sh", "echo 'error: it broke' >&2\nexit 1\n");
        let builds = Utf8TempDir::new().unwrap();

        let (built, lines) = build_in(&fixture, builds.path(), "0.1.0", Profile::Release);

        assert_eq!(
            built,
            Err(
                "`plugins.command.release.options.build` failed building 0.1.0: exit status: 1."
                    .to_owned()
            )
        );
        assert_eq!(lines[1..], ["error: it broke"]);
        assert_eq!(names(builds.path()), Vec::<String>::new());
    }

    /// The earlier build stays, since the new one can't be kept.
    #[test]
    fn a_build_that_wrote_the_wrong_commit_keeps_nothing() {
        let fixture = finalized_0_1_0();
        fixture.write("build.sh", &script(None));
        let builds = Utf8TempDir::new().unwrap();
        let tagged = fixture.hash("v0.1.0^{commit}");
        build_in(&fixture, builds.path(), "0.1.0", Profile::Release)
            .0
            .unwrap();
        fixture.write("build.sh", &script(Some(COMMIT)));

        let (built, _) = build_in(&fixture, builds.path(), "0.1.0", Profile::Release);

        assert_eq!(
            built,
            Err(format!(
                "The build of 0.1.0 left what `jp release` can't accept, so nothing was kept:\n- \
                 manifest.json names commit `{COMMIT}`, but the build was of {tagged}."
            ))
        );
        assert_eq!(names(builds.path()), ["0.1.0"]);
        assert_eq!(kept(builds.path(), "0.1.0").commit, tagged);
    }
}
