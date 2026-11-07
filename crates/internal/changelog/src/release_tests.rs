use std::{cell::Cell, process::Command};

use camino::Utf8PathBuf;
use camino_tempfile::Utf8TempDir;
use indoc::indoc;

use super::*;
use crate::forge::CreditKind;

/// 2026-10-01 at noon UTC, as seconds since the epoch: the first commit is a
/// day later.
const START: u64 = 1_790_856_000;

/// A day, in seconds.
const DAY: u64 = 86_400;

/// A repository a test writes history into.
///
/// Git runs without the machine's configuration, so a signing setup or a hook
/// can't reach in, and every commit and tag is made a day after the last, in
/// UTC+2.
struct Repository {
    dir: Utf8TempDir,

    /// Holds the empty file git reads as its global configuration.
    home: Utf8TempDir,

    /// How many commits and tags were made.
    days: Cell<u64>,
}

impl Repository {
    /// A repository on `main` whose `origin` is `origin`, with an empty
    /// fragments directory.
    fn new(origin: Option<&str>) -> Self {
        let home = Utf8TempDir::new().unwrap();
        fs::write(home.path().join("gitconfig"), "").unwrap();
        let repository = Self {
            dir: Utf8TempDir::new().unwrap(),
            home,
            days: Cell::new(0),
        };

        repository.git(&[
            "init",
            "--quiet",
            "--initial-branch=main",
            "--object-format=sha1",
        ]);
        if let Some(origin) = origin {
            repository.git(&["remote", "add", "origin", origin]);
        }
        fs::create_dir_all(repository.fragments()).unwrap();

        repository
    }

    fn path(&self) -> &Utf8Path {
        self.dir.path()
    }

    fn fragments(&self) -> Utf8PathBuf {
        self.path().join("changelog.d")
    }

    /// Write `contents` to `path`, below the repository's root.
    fn write(&self, path: &str, contents: &str) {
        let path = self.path().join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }

    /// Commit every change in the work tree with `message`.
    fn commit(&self, message: &str) {
        self.git(&["add", "--all"]);
        self.dated(&["commit", "--quiet", "--message", message]);
    }

    /// Move each of `paths`, below the fragments directory, into the release's
    /// directory `version`.
    fn file_into(&self, version: &str, paths: &[&str]) {
        for path in paths {
            let to = format!("changelog.d/{version}/{path}");
            fs::create_dir_all(self.path().join(&to).parent().unwrap()).unwrap();
            self.git(&["mv", &format!("changelog.d/{path}"), &to]);
        }
    }

    /// Tag `HEAD` as `name`, with an annotated tag.
    fn tag(&self, name: &str) {
        self.dated(&["tag", "--annotate", name, "--message", name]);
    }

    /// The full hash of `revision`.
    fn hash(&self, revision: &str) -> String {
        self.git(&["rev-parse", revision]).trim().to_owned()
    }

    /// Run git with `args`, dated a day after the last dated run.
    fn dated(&self, args: &[&str]) -> String {
        let days = self.days.get() + 1;
        self.days.set(days);
        let date = format!("{} +0200", START + days * DAY);

        self.run(args, &[
            ("GIT_AUTHOR_DATE", &date),
            ("GIT_COMMITTER_DATE", &date),
        ])
    }

    fn git(&self, args: &[&str]) -> String {
        self.run(args, &[])
    }

    fn run(&self, args: &[&str], env: &[(&str, &str)]) -> String {
        let output = Command::new("git")
            .current_dir(self.path())
            .args(args)
            .env("GIT_CONFIG_GLOBAL", self.home.path().join("gitconfig"))
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "Tester")
            .env("GIT_AUTHOR_EMAIL", "tester@example.com")
            .env("GIT_COMMITTER_NAME", "Tester")
            .env("GIT_COMMITTER_EMAIL", "tester@example.com")
            .envs(env.iter().copied())
            .output()
            .unwrap_or_else(|error| panic!("git {args:?}: {error}"));

        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    }
}

/// The repository JP's own would look like after cutting 0.3.0, and merging a
/// change and a correction since.
fn jp_after_cutting_0_3_0() -> Repository {
    let repository = Repository::new(Some("git@github.com:dcdpr/jp.git"));

    repository.write(
        "changelog.d/fan-out.added.md",
        "The assistant can call one tool on many inputs in a single request.\n",
    );
    repository.write(
        "changelog.d/highlights/fan-out.md",
        "# Tool calls fan out\n\n- **Authors**: @JeanMertz\n\nThe assistant asks once, and JP \
         runs the calls side by side.\n",
    );
    repository.commit("feat(tools): Fan tool calls out (#1182)");

    repository.write(
        "changelog.d/highlights/fan-out.md",
        "# Tool calls fan out\n\n- **Authors**: @JeanMertz\n\nThe assistant asks once, and JP \
         runs the calls side by side, MCP tools included.\n",
    );
    repository.commit("feat(mcp): Fan MCP tool calls out (#1183)");

    repository.write(
        "changelog.d/retry-idle-turns.fixed.md",
        "A turn that stalls while idle is retried instead of fialing.\n",
    );
    repository.commit("fix(llm): Retry idle turns (#1207)");

    repository.write(
        "changelog.d/help-default.fixed.md",
        "The help for `jp query --tmp` names the right default.\n",
    );
    repository.commit("Name the right default in the help");

    repository.write("changelog.d/plugin-manifest-routing.changed.md", indoc! {"
            JP finds a command plugin by the manifest its binary embeds.

            BREAKING CHANGE: Command plugins are found by their manifest

            Rebuild the plugin, or reinstall it.
        "});
    repository.commit("feat(plugin)!: Route plugins by manifest (#1216)");

    repository.file_into("0.3.0", &[
        "fan-out.added.md",
        "highlights/fan-out.md",
        "retry-idle-turns.fixed.md",
        "help-default.fixed.md",
        "plugin-manifest-routing.changed.md",
    ]);
    repository.commit("chore(release): Cut 0.3.0 (#1250)");

    repository.write(
        "changelog.d/0.3.0/retry-idle-turns.fixed.md",
        "A turn that stalls while idle is retried instead of failing.\n",
    );
    repository.commit("docs(changelog): Correct a typo (#1260)");

    repository.write(
        "changelog.d/show-model.added.md",
        "`jp conversation show` shows the conversation's model.\n",
    );
    repository.commit("feat(cli): Show the conversation's model (#1243)");

    repository
}

fn release(version: &str) -> Part {
    Part::Release(Version::parse(version).unwrap())
}

/// Each entry's slug, and the labels of its credits.
fn entry_credits(release: &Release) -> Vec<(&str, Vec<&str>)> {
    release
        .entries
        .iter()
        .map(|entry| {
            let labels = entry.credits.iter().map(|credit| credit.label.as_str());
            (entry.slug.as_str(), labels.collect())
        })
        .collect()
}

#[test]
fn each_file_is_credited_to_the_pull_requests_that_wrote_it_before_the_cut() {
    let repository = jp_after_cutting_0_3_0();

    let release = read(
        &repository.fragments(),
        &release("0.3.0"),
        None,
        &Headings::default(),
    )
    .unwrap();

    // The cut (#1250) and the correction after it (#1260) credit nobody.
    let help = repository.hash("HEAD~4");
    assert_eq!(entry_credits(&release), [
        ("fan-out", vec!["#1182"]),
        ("plugin-manifest-routing", vec!["#1216"]),
        ("retry-idle-turns", vec!["#1207"]),
        ("help-default", vec![&help[..7]]),
    ]);
    // A highlight is credited to every pull request that edited it.
    let highlight = &release.highlights[0];
    assert_eq!(highlight.slug, "fan-out");
    let labels: Vec<&str> = highlight
        .credits
        .iter()
        .map(|credit| credit.label.as_str())
        .collect();
    assert_eq!(labels, ["#1182", "#1183"]);
}

/// A commit whose message names no pull request is credited as itself.
#[test]
fn a_commit_without_a_pull_request_is_credited_as_itself() {
    let repository = jp_after_cutting_0_3_0();

    let release = read(
        &repository.fragments(),
        &release("0.3.0"),
        None,
        &Headings::default(),
    )
    .unwrap();

    let help = repository.hash("HEAD~4");
    let entry = &release.entries[3];
    assert_eq!(entry.slug, "help-default");
    assert_eq!(entry.credits, [Credit {
        kind: CreditKind::Commit,
        label: help[..7].to_owned(),
        url: Some(format!("https://github.com/dcdpr/jp/commit/{help}")),
    }]);
}

#[test]
fn a_release_is_dated_by_its_tag() {
    let repository = jp_after_cutting_0_3_0();
    let fragments = repository.fragments();

    let untagged = read(&fragments, &release("0.3.0"), None, &Headings::default()).unwrap();
    assert_eq!(untagged.version, Some(Version::new(0, 3, 0)));
    assert_eq!(untagged.date, None);

    // Eight commits, then the tag, a day after each other from October 1st.
    repository.tag("v0.3.0");
    let tagged = read(&fragments, &release("0.3.0"), None, &Headings::default()).unwrap();
    assert_eq!(tagged.date.as_deref(), Some("2026-10-10"));
}

#[test]
fn the_unreleased_fragments_have_no_version_or_date() {
    let repository = jp_after_cutting_0_3_0();
    repository.tag("v0.3.0");

    let unreleased = read(
        &repository.fragments(),
        &Part::Unreleased,
        None,
        &Headings::default(),
    )
    .unwrap();

    assert_eq!(unreleased.version, None);
    assert_eq!(unreleased.date, None);
    assert_eq!(entry_credits(&unreleased), [("show-model", vec!["#1243"])]);
    assert_eq!(unreleased.overview, "1 entry is unreleased: Added (1).");
}

#[test]
fn without_an_overview_the_release_is_summarized() {
    let repository = jp_after_cutting_0_3_0();
    let headings = Headings::from_iter([
        (Type::Added, "New features".to_owned()),
        (Type::Fixed, "Bug fixes".to_owned()),
    ]);

    let release = read(&repository.fragments(), &release("0.3.0"), None, &headings).unwrap();

    assert_eq!(release.overview, indoc! {"
        Version 0.3.0 has 4 entries: New features (1), Changed (1), and Bug fixes (2).

        Highlights:

        - Tool calls fan out"});
}

#[test]
fn the_overview_is_what_the_releases_overview_md_holds() {
    let repository = jp_after_cutting_0_3_0();
    repository.write(
        "changelog.d/0.3.0/overview.md",
        "JP 0.3.0 lets the assistant call one tool on [many inputs][fan-out].\n\n[fan-out]: \
         https://jp.computer/fan-out\n",
    );

    let release = read(
        &repository.fragments(),
        &release("0.3.0"),
        None,
        &Headings::default(),
    )
    .unwrap();

    assert_eq!(
        release.overview,
        "JP 0.3.0 lets the assistant call one tool on [many inputs](https://jp.computer/fan-out)."
    );
}

#[test]
fn an_empty_overview_md_is_summarized_like_a_missing_one() {
    let repository = jp_after_cutting_0_3_0();
    repository.write("changelog.d/0.3.0/overview.md", "\n  \n");

    let release = read(
        &repository.fragments(),
        &release("0.3.0"),
        None,
        &Headings::default(),
    )
    .unwrap();

    assert!(
        release.overview.starts_with("Version 0.3.0 has 4 entries"),
        "{}",
        release.overview
    );
}

/// A copy a patch release took lists its credits, a commit among them, and each
/// is credited once.
#[test]
fn listed_credits_replace_the_ones_history_names() {
    let repository = Repository::new(Some("https://github.com/dcdpr/jp"));
    repository.write("changelog.d/0.3.1/retry-idle-turns.fixed.md", indoc! {"
        - **Credits**: #1207
        - **Credits**: #1211
        - **Credits**: #1207
        - **Credits**: 4e1f0a2

        Idle turns are retried.
    "});
    repository.commit("chore(release): Pick a fix into 0.3.1 (#1300)");

    let release = read(
        &repository.fragments(),
        &release("0.3.1"),
        None,
        &Headings::default(),
    )
    .unwrap();

    assert_eq!(release.entries[0].credits, [
        Credit {
            kind: CreditKind::ChangeRequest,
            label: "#1207".to_owned(),
            url: Some("https://github.com/dcdpr/jp/pull/1207".to_owned()),
        },
        Credit {
            kind: CreditKind::ChangeRequest,
            label: "#1211".to_owned(),
            url: Some("https://github.com/dcdpr/jp/pull/1211".to_owned()),
        },
        Credit {
            kind: CreditKind::Commit,
            label: "4e1f0a2".to_owned(),
            url: Some("https://github.com/dcdpr/jp/commit/4e1f0a2".to_owned()),
        },
    ]);
}

/// Within a type, entries are ordered by their first credit, change requests by
/// number before commits by age, and then by slug.
#[test]
fn entries_are_ordered_by_type_then_first_credit_then_slug() {
    let repository = Repository::new(Some("https://github.com/dcdpr/jp"));
    repository.write(
        "changelog.d/direct.fixed.md",
        "Pushed without a pull request.\n",
    );
    repository.commit("Fix a thing directly");
    repository.write(
        "changelog.d/zeta.fixed.md",
        "The later of two in one pull request.\n",
    );
    repository.write(
        "changelog.d/alpha.fixed.md",
        "The earlier of two in one pull request.\n",
    );
    repository.commit("fix: Two fixes (#20)");
    repository.write(
        "changelog.d/early.fixed.md",
        "A lower number, merged later.\n",
    );
    repository.write("changelog.d/feature.added.md", "Added things come first.\n");
    repository.commit("fix: An older pull request (#9)");
    repository.write(
        "changelog.d/uncommitted.fixed.md",
        "Not committed yet, so nobody is credited.\n",
    );

    let release = read(
        &repository.fragments(),
        &Part::Unreleased,
        None,
        &Headings::default(),
    )
    .unwrap();

    let slugs: Vec<&str> = release
        .entries
        .iter()
        .map(|entry| entry.slug.as_str())
        .collect();
    assert_eq!(slugs, [
        "feature",
        "early",
        "alpha",
        "zeta",
        "direct",
        "uncommitted"
    ]);
}

#[test]
fn without_an_origin_every_credit_is_a_commit_linked_nowhere() {
    let repository = Repository::new(None);
    repository.write(
        "changelog.d/retry-idle-turns.fixed.md",
        "Idle turns are retried.\n",
    );
    repository.commit("fix(llm): Retry idle turns (#1207)");

    let release = read(
        &repository.fragments(),
        &Part::Unreleased,
        None,
        &Headings::default(),
    )
    .unwrap();

    let head = repository.hash("HEAD");
    assert_eq!(release.entries[0].credits, [Credit {
        kind: CreditKind::Commit,
        label: head[..7].to_owned(),
        url: None,
    }]);
}

#[test]
fn a_forge_that_links_needs_an_origin_with_a_web_address() {
    let repository = Repository::new(None);

    let error = read(
        &repository.fragments(),
        &Part::Unreleased,
        Some(Forge::GitHub),
        &Headings::default(),
    )
    .unwrap_err();

    assert!(matches!(error, ReleaseError::NoAddress(_)), "{error:?}");
}

/// A self-hosted GitLab names its forge, and a merge request is read from the
/// merge commit that brought its branch in.
#[test]
fn gitlab_merge_requests_are_read_from_merge_commits() {
    let repository = Repository::new(Some("git@git.example.com:group/project.git"));
    repository.dated(&["commit", "--quiet", "--allow-empty", "--message", "Start"]);
    repository.git(&["checkout", "--quiet", "-b", "retry"]);
    repository.write(
        "changelog.d/retry-idle-turns.fixed.md",
        "Idle turns are retried.\n",
    );
    repository.commit("Retry idle turns");
    repository.git(&["checkout", "--quiet", "main"]);
    repository.dated(&[
        "merge",
        "--quiet",
        "--no-ff",
        "retry",
        "--message",
        "Merge branch 'retry' into 'main'\n\nRetry idle turns\n\nSee merge request \
         group/project!42",
    ]);

    let release = read(
        &repository.fragments(),
        &Part::Unreleased,
        Some(Forge::GitLab),
        &Headings::default(),
    )
    .unwrap();

    assert_eq!(release.entries[0].credits, [Credit {
        kind: CreditKind::ChangeRequest,
        label: "!42".to_owned(),
        url: Some("https://git.example.com/group/project/-/merge_requests/42".to_owned()),
    }]);
}

#[test]
fn a_shallow_clone_is_refused() {
    let repository = jp_after_cutting_0_3_0();
    let clone = Utf8TempDir::new().unwrap();
    // Only a `file://` address clones shallow, and its path has to start with
    // `/`, which a Windows path does not.
    let path = repository.path().as_str().replace('\\', "/");
    let address = format!("file:///{}", path.trim_start_matches('/'));
    repository.git(&[
        "clone",
        "--quiet",
        "--depth=1",
        &address,
        clone.path().as_str(),
    ]);

    let error = read(
        &clone.path().join("changelog.d"),
        &release("0.3.0"),
        None,
        &Headings::default(),
    )
    .unwrap_err();

    assert!(matches!(error, ReleaseError::Shallow), "{error:?}");
    assert_eq!(
        error.to_string(),
        "the repository is a shallow clone, which lacks the history credits are read from; fetch \
         the rest of it with `git fetch --unshallow`"
    );
}

#[test]
fn a_part_that_breaks_the_format_is_not_read() {
    let repository = jp_after_cutting_0_3_0();
    repository.write("changelog.d/0.3.0/notes.txt", "Remember the overview.\n");

    let error = read(
        &repository.fragments(),
        &release("0.3.0"),
        None,
        &Headings::default(),
    )
    .unwrap_err();

    assert!(
        matches!(&error, ReleaseError::Read(ReadError::Problems(problems)) if problems.len() == 1),
        "{error:?}"
    );
}

/// The shape other tools read: every field the format holds, a `null` for what
/// is missing, and the type's id rather than its heading.
#[test]
fn a_release_serializes_to_the_json_other_tools_read() {
    let repository = Repository::new(Some("https://github.com/dcdpr/jp"));
    repository.write(
        "changelog.d/highlights/fan-out.md",
        "# Tool calls fan out\n\n- **Authors**: @JeanMertz\n\nThe assistant asks once.\n",
    );
    repository.write("changelog.d/plugin-manifest-routing.changed.md", indoc! {"
            JP finds a command plugin by its manifest.

            BREAKING CHANGE: Command plugins are found by their manifest

            Rebuild the plugin.
        "});
    repository.commit("feat(plugin)!: Route plugins by manifest (#1216)");
    repository.write(
        "changelog.d/retry-idle-turns.fixed.md",
        "Idle turns are retried.\n",
    );
    repository.commit("fix(llm): Retry idle turns (#1207)");
    repository.file_into("0.3.0", &[
        "highlights/fan-out.md",
        "plugin-manifest-routing.changed.md",
        "retry-idle-turns.fixed.md",
    ]);
    repository.write("changelog.d/0.3.0/overview.md", "JP 0.3.0 fans out.\n");
    repository.commit("chore(release): Cut 0.3.0 (#1250)");
    repository.tag("v0.3.0");

    let release = read(
        &repository.fragments(),
        &release("0.3.0"),
        None,
        &Headings::default(),
    )
    .unwrap();

    assert_eq!(
        serde_json::to_string_pretty(&release).unwrap(),
        indoc! {r##"
            {
              "format": 1,
              "version": "0.3.0",
              "date": "2026-10-05",
              "overview": "JP 0.3.0 fans out.",
              "highlights": [
                {
                  "slug": "fan-out",
                  "title": "Tool calls fan out",
                  "authors": [
                    "@JeanMertz"
                  ],
                  "body": "The assistant asks once.",
                  "credits": [
                    {
                      "kind": "change_request",
                      "label": "#1216",
                      "url": "https://github.com/dcdpr/jp/pull/1216"
                    }
                  ]
                }
              ],
              "entries": [
                {
                  "slug": "plugin-manifest-routing",
                  "type": "changed",
                  "body": "JP finds a command plugin by its manifest.",
                  "credits": [
                    {
                      "kind": "change_request",
                      "label": "#1216",
                      "url": "https://github.com/dcdpr/jp/pull/1216"
                    }
                  ],
                  "upgrade": {
                    "title": "Command plugins are found by their manifest",
                    "body": "Rebuild the plugin."
                  }
                },
                {
                  "slug": "retry-idle-turns",
                  "type": "fixed",
                  "body": "Idle turns are retried.",
                  "credits": [
                    {
                      "kind": "change_request",
                      "label": "#1207",
                      "url": "https://github.com/dcdpr/jp/pull/1207"
                    }
                  ],
                  "upgrade": null
                }
              ]
            }"##}
    );
}

#[test]
fn a_type_without_a_heading_is_headed_by_its_id_capitalized() {
    let headings = Headings::from_iter([(Type::Added, "New features".to_owned())]);

    assert_eq!(headings.of(Type::Added), "New features");
    assert_eq!(headings.of(Type::Performance), "Performance");
}

#[test]
fn a_series_lists_its_items_as_a_sentence_would() {
    let items =
        |items: &[&str]| -> Vec<String> { items.iter().map(|item| (*item).to_owned()).collect() };

    assert_eq!(series(&items(&[])), "");
    assert_eq!(series(&items(&["a"])), "a");
    assert_eq!(series(&items(&["a", "b"])), "a and b");
    assert_eq!(series(&items(&["a", "b", "c"])), "a, b, and c");
}
