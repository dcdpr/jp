//! Isolated git workspace and bare `origin` remote fixture for release pipeline
//! tests.

use std::{
    fs,
    process::{Command, Output},
};

use camino::Utf8Path;
use camino_tempfile::Utf8TempDir;

use crate::shell::Shell;

pub(crate) struct Fixture {
    work: Utf8TempDir,
    origin: Utf8TempDir,
    home: Utf8TempDir,
}

impl Fixture {
    pub(crate) fn new() -> Self {
        let home = Utf8TempDir::new().unwrap();
        fs::write(home.path().join("gitconfig"), "").unwrap();
        let fixture = Self {
            work: Utf8TempDir::new().unwrap(),
            origin: Utf8TempDir::new().unwrap(),
            home,
        };

        fixture.origin_git(&["init", "--quiet", "--bare", "--initial-branch=main"]);
        fixture.git(&["init", "--quiet", "--initial-branch=main"]);
        fixture.git(&["remote", "add", "origin", fixture.origin.path().as_str()]);
        fixture.write("changelog.d/README.md", "# Changelog fragments\n");
        fixture.write("src/lib.rs", "// The project's code.\n");
        fixture.commit("Start the project");
        fixture.push();

        fixture
    }

    pub(crate) fn root(&self) -> &Utf8Path {
        self.work.path()
    }

    pub(crate) fn origin(&self) -> &Utf8Path {
        self.origin.path()
    }

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

    fn env_at(&self, at: i64) -> Vec<(String, String)> {
        let date = format!("{at} +0000");
        let mut env = self.env();
        env.push(("GIT_AUTHOR_DATE".to_owned(), date.clone()));
        env.push(("GIT_COMMITTER_DATE".to_owned(), date));

        env
    }

    pub(crate) fn shell(&self) -> Shell {
        Shell::with_env(self.env())
    }

    pub(crate) fn shell_at(&self, at: i64) -> Shell {
        Shell::with_env(self.env_at(at))
    }

    pub(crate) fn write(&self, path: &str, contents: &str) {
        let path = self.root().join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }

    pub(crate) fn fragment(&self, slug: &str, type_: &str, entry: &str) {
        self.write(
            &format!("changelog.d/{slug}.{type_}.md"),
            &format!("{entry}\n"),
        );
    }

    pub(crate) fn commit(&self, message: &str) -> String {
        self.git(&["add", "--all"]);
        self.git(&["commit", "--quiet", "--allow-empty", "--message", message]);
        self.hash("HEAD")
    }

    pub(crate) fn push(&self) {
        self.git(&["push", "--quiet", "origin", "main"]);
    }

    pub(crate) fn hash(&self, revision: &str) -> String {
        self.git(&["rev-parse", revision]).trim().to_owned()
    }

    pub(crate) fn show(&self, revision: &str) -> Option<String> {
        let output = Self::command(self.root(), &["show", revision], &self.env());

        output
            .status
            .success()
            .then(|| String::from_utf8(output.stdout).unwrap())
    }

    pub(crate) fn message(&self, revision: &str) -> String {
        self.git(&["log", "-1", "--format=%B", revision])
    }

    pub(crate) fn branches(&self) -> Vec<String> {
        Self::names(&self.git(&["for-each-ref", "--format=%(refname:short)", "refs/heads/"]))
    }

    pub(crate) fn origin_branches(&self) -> Vec<String> {
        Self::names(&self.origin_git(&["for-each-ref", "--format=%(refname:short)", "refs/heads/"]))
    }

    pub(crate) fn origin_tags(&self) -> Vec<String> {
        Self::names(&self.origin_git(&["tag", "--list"]))
    }

    fn names(listing: &str) -> Vec<String> {
        listing.lines().map(ToOwned::to_owned).collect()
    }

    pub(crate) fn publish(&self, refs: &[&str]) {
        let mut args = vec!["push", "--quiet", "origin"];
        args.extend(refs);
        self.git(&args);
    }

    pub(crate) fn merge_notes(&self, version: &str, message: &str) -> String {
        self.git(&[
            "merge",
            "--quiet",
            "--squash",
            &format!("release-notes/v{version}"),
        ]);
        let hash = self.commit(message);
        self.push();

        hash
    }

    pub(crate) fn git(&self, args: &[&str]) -> String {
        Self::git_in(self.root(), args, &self.env())
    }

    pub(crate) fn git_at(&self, at: i64, args: &[&str]) -> String {
        Self::git_in(self.root(), args, &self.env_at(at))
    }

    pub(crate) fn origin_git(&self, args: &[&str]) -> String {
        Self::git_in(self.origin(), args, &self.env())
    }

    fn git_in(dir: &Utf8Path, args: &[&str], env: &[(String, String)]) -> String {
        let output = Self::command(dir, args, env);
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );

        String::from_utf8(output.stdout).unwrap()
    }

    fn command(dir: &Utf8Path, args: &[&str], env: &[(String, String)]) -> Output {
        Command::new("git")
            .current_dir(dir)
            .args(args)
            .envs(env.iter().map(|(key, value)| (key, value)))
            .output()
            .unwrap_or_else(|error| panic!("git {args:?}: {error}"))
    }
}
