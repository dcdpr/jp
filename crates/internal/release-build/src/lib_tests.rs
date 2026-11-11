use camino_tempfile::Utf8TempDir;
use serde_json::json;

use super::*;

/// The commit the tests' builds are of.
const COMMIT: &str = "8a9b0c1d2e3f4a5b6c7d8e9f0a1b2c3d4e5f6a7b";

/// The archive the tests' builds write.
const ARCHIVE: &str = "jp-x86_64-unknown-linux-gnu.tar.xz";

/// What [`ARCHIVE`] holds.
const CONTENTS: &str = "an archive\n";

/// The SHA-256 of [`CONTENTS`], as `shasum -a 256` gives it.
const HASH: &str = "4cd36b3ca046f7c17e81e1aa31bf2dc6a91b98c94238810db57456b4904d3a22";

/// The platform [`ARCHIVE`] is for.
const TARGET: &str = "x86_64-unknown-linux-gnu";

fn version(text: &str) -> Version {
    Version::parse(text).unwrap()
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
        json!({
            "format": 1,
            "version": "0.3.0",
            "commit": COMMIT,
            "profile": "release",
            "artifacts": [{ "file": ARCHIVE, "target": TARGET }],
        })
        .to_string(),
    )
    .unwrap();

    out
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

/// A release can ship an installer beside an archive for the same platform.
#[test]
fn a_target_can_have_several_files() {
    let out = written();
    fs::write(out.path().join("jp-setup.exe"), CONTENTS).unwrap();
    fs::write(
        out.path().join("checksums.txt"),
        format!("{HASH}  {ARCHIVE}\n{HASH}  jp-setup.exe\n"),
    )
    .unwrap();
    fs::write(
        out.path().join("manifest.json"),
        json!({
            "format": 1,
            "version": "0.3.0",
            "commit": COMMIT,
            "profile": "release",
            "artifacts": [
                { "file": ARCHIVE, "target": TARGET },
                { "file": "jp-setup.exe", "target": TARGET },
            ],
        })
        .to_string(),
    )
    .unwrap();

    let manifest = check(out.path(), &version("0.3.0"), COMMIT, Profile::Release).unwrap();

    assert_eq!(manifest.artifacts.len(), 2);
}

/// A publisher reads a kept build and hashes its files again, so a file changed
/// since it was built is caught.
#[test]
fn a_file_changed_since_the_build_is_caught() {
    let out = written();
    let manifest = read_manifest(out.path()).unwrap();
    assert_eq!(check_files(out.path(), &manifest), Vec::<String>::new());

    fs::write(out.path().join(ARCHIVE), "another archive\n").unwrap();

    assert_eq!(check_files(out.path(), &manifest), [format!(
        "checksums.txt gives `{ARCHIVE}` the SHA-256 {HASH}, but its contents hash to \
         dbbbcf5e0ef77e03885d7d606eaf3d60cc8949ed10c3fb471cc271599c3a971e."
    )]);
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
        "manifest.json is format 2, and this plugin reads format 1."
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
