//! A workspace in a git repository whose `origin` is a bare repository beside
//! it, for the tests of `cut`, `pick`, and `finalize`.
//!
//! Git runs without the machine's configuration, both here and in the [`Shell`]
//! the fixture hands the code under test, so a signing setup or a hook can't
//! reach in.

use std::{
    fs,
    process::{Command, Output},
};

use camino::Utf8Path;
use camino_tempfile::Utf8TempDir;

use crate::shell::Shell;

/// A workspace and its `origin`.
pub(crate) struct Fixture {
    /// The workspace root, the top of its repository.
    work: Utf8TempDir,

    /// The bare repository the workspace's `origin` is.
    origin: Utf8TempDir,

    /// Holds the empty file git reads as its global configuration.
    home: Utf8TempDir,
}

impl Fixture {
    /// A workspace on `main` with an empty fragments directory, its first
    /// commit pushed to `origin`.
    pub(crate) fn new() -> Self {
        let home = Utf8TempDir::new().unwrap();
        fs::write(home.path().join("gitconfig"), "").unwrap();
        let fixture = Self {
            work: Utf8TempDir::new().unwrap(),
            origin: Utf8TempDir::new().unwrap(),
            home,
        };

        fixture.git_in(fixture.origin.path(), &[
            "init",
            "--quiet",
            "--bare",
            "--initial-branch=main",
        ]);
        fixture.git(&["init", "--quiet", "--initial-branch=main"]);
        fixture.git(&["remote", "add", "origin", fixture.origin.path().as_str()]);
        fixture.write("changelog.d/README.md", "# Changelog fragments\n");
        fixture.write("src/lib.rs", "// The project's code.\n");
        fixture.commit("Start the project");
        fixture.push();

        fixture
    }

    /// The workspace root.
    pub(crate) fn root(&self) -> &Utf8Path {
        self.work.path()
    }

    /// The bare repository `origin` is.
    pub(crate) fn origin(&self) -> &Utf8Path {
        self.origin.path()
    }

    /// The environment every git run here has, and the code under test runs
    /// programs with.
    fn env(&self) -> Vec<(String, String)> {
        [
            (
                "GIT_CONFIG_GLOBAL",
                self.home.path().join("gitconfig").as_str(),
            ),
            ("GIT_CONFIG_NOSYSTEM", "1"),
            ("GIT_AUTHOR_NAME", "Tester"),
            ("GIT_AUTHOR_EMAIL", "tester@example.com"),
            ("GIT_COMMITTER_NAME", "Tester"),
            ("GIT_COMMITTER_EMAIL", "tester@example.com"),
        ]
        .into_iter()
        .map(|(key, value)| (key.to_owned(), value.to_owned()))
        .collect()
    }

    /// The shell the code under test runs git and `bump` with.
    pub(crate) fn shell(&self) -> Shell {
        Shell::with_env(self.env())
    }

    /// Write `contents` to `path`, below the workspace root.
    pub(crate) fn write(&self, path: &str, contents: &str) {
        let path = self.root().join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }

    /// Write a fragment of type `type_` holding `entry`, named `slug`.
    pub(crate) fn fragment(&self, slug: &str, type_: &str, entry: &str) {
        self.write(
            &format!("changelog.d/{slug}.md"),
            &format!("---\ntype: {type_}\n---\n{entry}\n"),
        );
    }

    /// Commit every change in the work tree with `message`, and return the
    /// commit's hash.
    pub(crate) fn commit(&self, message: &str) -> String {
        self.git(&["add", "--all"]);
        self.git(&["commit", "--quiet", "--allow-empty", "--message", message]);
        self.hash("HEAD")
    }

    /// Push `main` to `origin`.
    pub(crate) fn push(&self) {
        self.git(&["push", "--quiet", "origin", "main"]);
    }

    /// The full hash of `revision` in the workspace's repository.
    pub(crate) fn hash(&self, revision: &str) -> String {
        self.git(&["rev-parse", revision]).trim().to_owned()
    }

    /// What `origin` holds at `revision`, such as `release/0.3:changelog.d`, or
    /// `None` when it holds nothing there.
    pub(crate) fn show(&self, revision: &str) -> Option<String> {
        let output = self.command(self.origin(), &["show", revision]);

        output
            .status
            .success()
            .then(|| String::from_utf8(output.stdout).unwrap())
    }

    /// The message of the commit `origin` has at `revision`.
    pub(crate) fn message(&self, revision: &str) -> String {
        self.git_in(self.origin(), &["log", "-1", "--format=%B", revision])
    }

    /// Run git with `args` in the workspace root, and return what it printed.
    pub(crate) fn git(&self, args: &[&str]) -> String {
        self.git_in(self.root(), args)
    }

    /// Run git with `args` in `origin`, and return what it printed.
    pub(crate) fn origin_git(&self, args: &[&str]) -> String {
        self.git_in(self.origin(), args)
    }

    fn git_in(&self, dir: &Utf8Path, args: &[&str]) -> String {
        let output = self.command(dir, args);
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );

        String::from_utf8(output.stdout).unwrap()
    }

    fn command(&self, dir: &Utf8Path, args: &[&str]) -> Output {
        Command::new("git")
            .current_dir(dir)
            .args(args)
            .envs(self.env())
            .output()
            .unwrap_or_else(|error| panic!("git {args:?}: {error}"))
    }
}
