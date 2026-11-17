//! End-to-end tests for `code_symbol` and `code_rename` against a real
//! `rust-analyzer` process.
//!
//! Each test spawns `rust-analyzer` on a temporary fixture crate.
//! Because `rust-analyzer` is included in the pinned Rust toolchain, a missing
//! binary fails the test rather than skipping it.

use std::{
    fs, panic, process,
    sync::{
        Once,
        atomic::{AtomicUsize, Ordering},
    },
};

use camino::{Utf8Path, Utf8PathBuf};
use camino_tempfile::Utf8TempDir;
use jp_tool::{Action, Context, Outcome};
use pretty_assertions::assert_eq;
use serde_json::{Map, Value, json};
use tools::Tool;

const LIB: &str = "\
pub mod stream;
pub use stream::Stream;

/// Renders a value.
pub trait Render {
    /// Render it.
    fn render(&self) -> String;
}

pub struct Other;

impl Other {
    pub fn iter(&self) {}
}
";

const STREAM: &str = "\
use crate::Render;

/// A stream of numbers.
///
/// Second paragraph.
#[derive(Debug, Default)]
pub struct Stream {
    /// The items.
    pub items: Vec<u32>,
    pub config: String,
}

impl Stream {
    /// Iterate over the items.
    pub fn iter(&self) -> impl Iterator<Item = &u32> {
        self.items.iter()
    }
}

impl Render for Stream {
    fn render(&self) -> String {
        let total: u32 = self.iter().sum();
        format!(\"{total} {}\", self.config)
    }
}

pub fn total(stream: &Stream) -> u32 {
    stream.iter().sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts() {
        let stream = Stream::default();
        assert_eq!(stream.iter().count(), 0);
    }
}

#[cfg(test)]
#[path = \"stream_tests.rs\"]
mod more_tests;
";

const STREAM_TESTS: &str = "\
use super::*;

#[test]
fn renders() {
    let stream = Stream::default();
    assert_eq!(stream.iter().count(), 0);
    assert_eq!(stream.config, \"\");
}
";

fn fixture() -> Utf8TempDir {
    let dir = camino_tempfile::tempdir().unwrap();
    let root = dir.path();
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[workspace]\n",
    )
    .unwrap();
    fs::write(root.join("src/lib.rs"), LIB).unwrap();
    fs::write(root.join("src/stream.rs"), STREAM).unwrap();
    fs::write(root.join("src/stream_tests.rs"), STREAM_TESTS).unwrap();
    dir
}

/// Fixture exercising the reference categories documented in `symbol.toml`.
const COVERAGE: &str = r#"pub struct Point {
    pub x: u32,
    pub y: u32,
}

impl Point {
    pub fn new(x: u32, y: u32) -> Self {
        Self { x, y }
    }

    pub fn unused(&self) {}
}

pub fn literal() -> Point {
    Point { x: 1, y: 2 }
}

pub fn assign(p: &mut Point) {
    p.x = 3;
    p.y += 1;
}

pub fn destructure(p: &Point) -> u32 {
    let Point { x, .. } = p;
    *x
}

pub fn in_format(p: &Point) -> String {
    format!("{}", p.y)
}

macro_rules! get_x {
    ($p:expr) => {
        $p.x
    };
}

pub fn via_macro(p: &Point) -> u32 {
    get_x!(p)
}

#[derive(Debug, Clone)]
pub struct Derived {
    pub z: u32,
}

pub trait Shape {
    fn area(&self) -> u32;
}

impl Shape for Point {
    fn area(&self) -> u32 {
        self.x * self.y
    }
}

pub fn generic<T: Shape>(t: &T) -> u32 {
    t.area()
}

pub fn dynamic(t: &dyn Shape) -> u32 {
    t.area()
}
"#;

fn coverage_fixture() -> Utf8TempDir {
    let dir = camino_tempfile::tempdir().unwrap();
    let root = dir.path();
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"coverage\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[workspace]\n",
    )
    .unwrap();
    fs::write(root.join("src/lib.rs"), COVERAGE).unwrap();
    dir
}

fn ctx(root: &Utf8Path) -> Context {
    Context {
        // rust-analyzer reports canonical paths, and macOS temp directories sit
        // behind a symlink.
        root: root.canonicalize_utf8().unwrap(),
        action: Action::Run,
        access: None,
        workspace_id: "test".into(),
        conversation_id: "test".into(),
    }
}

/// The most lines of each server log a failing test prints, from the end.
const LOG_TAIL: usize = 400;

/// Where this test process keeps its rust-analyzer logs.
fn log_dir() -> Utf8PathBuf {
    Utf8Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("code-tests")
        .join(process::id().to_string())
}

/// Tool options that have rust-analyzer write its log to a file of its own.
///
/// A test that fails prints the end of every log its process wrote, so a wrong
/// answer on CI comes with what the server was doing when it gave it.
fn logged_options() -> Map<String, Value> {
    static HOOK: Once = Once::new();
    static NEXT: AtomicUsize = AtomicUsize::new(0);

    HOOK.call_once(|| {
        let previous = panic::take_hook();
        panic::set_hook(Box::new(move |info| {
            print_logs(&log_dir());
            previous(info);
        }));
    });

    let path = log_dir().join(format!(
        "rust-analyzer-{}.log",
        NEXT.fetch_add(1, Ordering::SeqCst)
    ));
    let mut options = Map::new();
    options.insert("server_log".to_owned(), json!(path.as_str()));
    options
}

#[expect(
    clippy::print_stderr,
    reason = "the log is for whoever reads the failure"
)]
fn print_logs(dir: &Utf8Path) {
    let Ok(entries) = dir.read_dir_utf8() else {
        return;
    };
    let mut paths = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path().to_owned())
        .collect::<Vec<_>>();
    paths.sort();

    for path in paths {
        let text = fs::read_to_string(&path).unwrap_or_default();
        // Each entry starts with a timestamp. The lines that do not are the
        // server's configuration, which it dumps at start over several
        // hundred lines.
        let lines = text
            .lines()
            .filter(|line| line.starts_with(|c: char| c.is_ascii_digit()))
            .collect::<Vec<_>>();
        let tail = &lines[lines.len().saturating_sub(LOG_TAIL)..];
        eprintln!(
            "\n==== {path}: last {} of {} entries ====\n{}",
            tail.len(),
            lines.len(),
            tail.join("\n")
        );
    }
}

fn tool(arguments: &Value) -> Tool {
    Tool {
        name: "code_symbol".to_owned(),
        arguments: arguments.as_object().unwrap().clone(),
        answers: Map::new(),
        options: logged_options(),
    }
}

/// Narrowing `Stream::iter` references with `pattern: "config"` excludes items
/// that do not match and marks the matching line with `=`.
#[tokio::test]
async fn a_pattern_keeps_the_items_it_matches_in() {
    let dir = fixture();
    let outcome = tools::run(
        ctx(dir.path()),
        tool(&json!({ "targets": [{ "name": "Stream::iter" }], "pattern": "config" })),
    )
    .await
    .unwrap();

    let Outcome::Success { content } = outcome else {
        panic!("{outcome:?}");
    };
    assert_eq!(
        content,
        "\
fixture::stream::Stream::iter (src/stream.rs:15)
/// Iterate over the items.
pub fn iter(&self) -> impl Iterator<Item = &u32>

References: 4 in 2 files: 2 in 1 file outside tests, 2 in 2 files in tests. 2 in 2 items where \
         `config` also matches.
src/stream.rs (1)
in fn <Stream as Render>::render (21-24):
21-fn render(&self) -> String {
22:    let total: u32 = self.iter().sum();
23=    format!(\"{total} {}\", self.config)
24-}
In tests, 1 in 1 file. Pass `tests: true` to show them:
src/stream_tests.rs (1)"
    );
}

const CALLS: &str = "\
pub fn leaf() -> u32 {
    1
}

pub fn middle() -> u32 {
    leaf() + leaf()
}

pub fn top() -> u32 {
    middle() + Some(2).unwrap_or(0)
}

pub fn other() -> u32 {
    leaf()
}

pub struct Pair(pub u32, pub u32);

pub enum Shape {
    Circle(u32),
}

pub fn build() -> (Pair, Shape) {
    (Pair(1, 2), Shape::Circle(2))
}

#[derive(Clone)]
pub struct Twin(pub u32);

pub fn twin(t: &Twin) -> Twin {
    t.clone()
}

#[cfg(test)]
mod tests {
    #[test]
    fn uses_leaf() {
        assert_eq!(super::leaf(), 1);
    }
}
";

fn calls_fixture() -> Utf8TempDir {
    let dir = camino_tempfile::tempdir().unwrap();
    let root = dir.path();
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"calls\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[workspace]\n",
    )
    .unwrap();
    fs::write(root.join("src/lib.rs"), CALLS).unwrap();
    dir
}

/// Verify `callers` (up to two levels, counting test callers) and `calls`
/// (distinguishing workspace functions, external functions, derived methods,
/// and struct/variant constructors).
#[tokio::test]
async fn shows_callers_up_the_tree_and_calls_down() {
    let dir = calls_fixture();
    let outcome = tools::run(
        ctx(dir.path()),
        tool(&json!({
            "targets": [
                { "name": "leaf" },
                { "name": "top" },
                { "name": "build" },
                { "name": "twin" },
            ],
            "show": ["callers", "calls"],
        })),
    )
    .await
    .unwrap();

    let Outcome::Success { content } = outcome else {
        panic!("{outcome:?}");
    };
    assert_eq!(
        content.as_str(),
        "\
calls::leaf (src/lib.rs:1)
pub fn leaf() -> u32

Callers, 2 levels up:
fn middle (src/lib.rs:6), 2 calls
  fn top (src/lib.rs:10)
fn other (src/lib.rs:14)
In tests, 1 test function. Pass `tests: true` to show them.

Calls: none.

---

calls::top (src/lib.rs:9)
pub fn top() -> u32

Callers: none in the workspace.

Calls: 2, 1 outside the workspace.
fn middle (src/lib.rs:5), at 10
Outside the workspace: unwrap_or

---

calls::build (src/lib.rs:23)
pub fn build() -> (Pair, Shape)

Callers: none in the workspace.

Calls: none.
Types and variants it names: struct Pair, variant Shape::Circle

---

calls::twin (src/lib.rs:30)
pub fn twin(t: &Twin) -> Twin

Callers: none in the workspace.

Calls: 1.
derived fn Twin::clone (src/lib.rs:27), at 31"
    );
}

fn rename_tool(arguments: &Value, answers: &Value) -> Tool {
    Tool {
        name: "code_rename".to_owned(),
        arguments: arguments.as_object().unwrap().clone(),
        answers: answers.as_object().unwrap().clone(),
        options: logged_options(),
    }
}

/// Renaming `Stream::iter` updates its definition and all resolved calls while
/// leaving `Vec::iter` (`self.items.iter()`) untouched.
#[tokio::test]
async fn renames_a_method_everywhere_it_is_called() {
    let dir = fixture();
    let arguments = json!({ "renames": [{ "name": "Stream::iter", "new_name": "values" }] });

    // Unanswered, the patch is put to the user and nothing is written.
    let asked = tools::run(ctx(dir.path()), rename_tool(&arguments, &json!({})))
        .await
        .unwrap();
    let Outcome::NeedsInput { question } = asked else {
        panic!("{asked:?}");
    };
    assert_eq!(question.id.as_str(), "apply_changes");
    assert_eq!(
        fs::read_to_string(dir.path().join("src/stream.rs")).unwrap(),
        STREAM
    );

    let outcome = tools::run(
        ctx(dir.path()),
        rename_tool(&arguments, &json!({ "apply_changes": true })),
    )
    .await
    .unwrap();
    let Outcome::Success { content } = outcome else {
        panic!("{outcome:?}");
    };
    assert_eq!(
        content.as_str(),
        "\
Renamed `Stream::iter` (src/stream.rs:15) to `values`: 5 edits in 2 files.
src/stream.rs (4)
src/stream_tests.rs (1)
rust-analyzer does not rename the name inside `macro_rules!` bodies, strings, comments, or \
         non-Rust files. Run `cargo_check`, and search for the old name with `fs_grep_files` \
         where those could hold it."
    );

    let stream = fs::read_to_string(dir.path().join("src/stream.rs")).unwrap();
    assert_eq!(
        stream,
        STREAM
            .replace("pub fn iter(&self)", "pub fn values(&self)")
            .replace("self.iter()", "self.values()")
            .replace("stream.iter()", "stream.values()")
    );
    assert!(stream.contains("self.items.iter()"), "{stream}");
    assert_eq!(
        fs::read_to_string(dir.path().join("src/stream_tests.rs")).unwrap(),
        STREAM_TESTS.replace("stream.iter()", "stream.values()")
    );
}

/// When two renames in a single call edit the same line, the second rename
/// resolves against the updated text with its target column shifted.
#[tokio::test]
async fn renames_in_a_batch_build_on_each_other() {
    let dir = coverage_fixture();
    let outcome = tools::run(
        ctx(dir.path()),
        rename_tool(
            &json!({ "renames": [
                { "name": "x", "path": "src/lib.rs", "line": 7, "new_name": "ax" },
                { "name": "y", "path": "src/lib.rs", "line": 7, "new_name": "by" },
            ] }),
            &json!({ "apply_changes": true }),
        ),
    )
    .await
    .unwrap();

    let Outcome::Success { content } = outcome else {
        panic!("{outcome:?}");
    };
    assert_eq!(
        content,
        "\
Renamed `x` (src/lib.rs:7) to `ax`: 2 edits in 1 file.
Renamed `y` (src/lib.rs:7) to `by`: 2 edits in 1 file.
src/lib.rs (4)
rust-analyzer does not rename the name inside `macro_rules!` bodies, strings, comments, or \
         non-Rust files. Run `cargo_check`, and search for the old name with `fs_grep_files` \
         where those could hold it."
    );
    assert_eq!(
        fs::read_to_string(dir.path().join("src/lib.rs")).unwrap(),
        COVERAGE.replace(
            "    pub fn new(x: u32, y: u32) -> Self {\n        Self { x, y }",
            "    pub fn new(ax: u32, by: u32) -> Self {\n        Self { x: ax, y: by }",
        )
    );
}

#[tokio::test]
async fn an_ambiguous_rename_lists_the_candidates() {
    let dir = fixture();
    let outcome = tools::run(
        ctx(dir.path()),
        rename_tool(
            &json!({ "renames": [{ "name": "iter", "new_name": "values" }] }),
            &json!({ "apply_changes": true }),
        ),
    )
    .await
    .unwrap();

    let Outcome::Error { message, .. } = outcome else {
        panic!("{outcome:?}");
    };
    assert_eq!(
        message,
        "`iter` names 2 items. Rename one at a time: give `path` and `line`:\n- src/lib.rs:13\n- \
         src/stream.rs:15"
    );
    assert_eq!(
        fs::read_to_string(dir.path().join("src/stream.rs")).unwrap(),
        STREAM
    );
}

#[tokio::test]
async fn finds_references_by_name_by_field_and_by_file() {
    let dir = fixture();
    let outcome = tools::run(
        ctx(dir.path()),
        tool(&json!({
            "targets": [
                { "name": "Stream::iter" },
                { "name": "iter" },
                { "name": "Stream::config" },
                { "name": "total", "path": "src/stream.rs" },
            ],
        })),
    )
    .await
    .unwrap();

    let Outcome::Success { content } = outcome else {
        panic!("{outcome:?}");
    };
    assert_eq!(
        content.as_str(),
        "\
fixture::stream::Stream::iter (src/stream.rs:15)
/// Iterate over the items.
pub fn iter(&self) -> impl Iterator<Item = &u32>

References: 4 in 2 files: 2 in 1 file outside tests, 2 in 2 files in tests.
src/stream.rs (2)
in fn <Stream as Render>::render (21-24):
21-fn render(&self) -> String {
22:    let total: u32 = self.iter().sum();
23-    format!(\"{total} {}\", self.config)
24-}
in fn total (27-29):
27-pub fn total(stream: &Stream) -> u32 {
28:    stream.iter().sum()
29-}
In tests, 2 in 2 files. Pass `tests: true` to show them:
src/stream.rs (1)
src/stream_tests.rs (1)

---

`iter` names 2 items; each is answered below.

fixture::Other::iter (src/lib.rs:13)
pub fn iter(&self)

References: none in the workspace.

---

fixture::stream::Stream::iter (src/stream.rs:15)
/// Iterate over the items.
pub fn iter(&self) -> impl Iterator<Item = &u32>

References: 4 in 2 files: 2 in 1 file outside tests, 2 in 2 files in tests.
src/stream.rs (2)
in fn <Stream as Render>::render (21-24):
21-fn render(&self) -> String {
22:    let total: u32 = self.iter().sum();
23-    format!(\"{total} {}\", self.config)
24-}
in fn total (27-29):
27-pub fn total(stream: &Stream) -> u32 {
28:    stream.iter().sum()
29-}
In tests, 2 in 2 files. Pass `tests: true` to show them:
src/stream.rs (1)
src/stream_tests.rs (1)

---

fixture::stream::Stream::config (src/stream.rs:10)
pub config: String

References: 2 in 2 files: 1 in 1 file outside tests, 1 in 1 file in tests.
src/stream.rs (1)
in fn <Stream as Render>::render (21-24):
21-fn render(&self) -> String {
22-    let total: u32 = self.iter().sum();
23:    format!(\"{total} {}\", self.config)
24-}
In tests, 1 in 1 file. Pass `tests: true` to show them:
src/stream_tests.rs (1)

---

fixture::stream::total (src/stream.rs:27)
pub fn total(stream: &Stream) -> u32

References: none in the workspace."
    );
}

#[tokio::test]
async fn shows_definitions_and_implementations() {
    let dir = fixture();
    let outcome = tools::run(
        ctx(dir.path()),
        tool(&json!({
            "targets": [
                // `stream.iter().sum()`, a call site.
                { "name": "iter", "path": "src/stream.rs", "line": 28 },
                { "name": "Render" },
            ],
            "show": ["definition", "implementations"],
        })),
    )
    .await
    .unwrap();

    let Outcome::Success { content } = outcome else {
        panic!("{outcome:?}");
    };
    assert_eq!(
        content.as_str(),
        "\
fixture::stream::Stream::iter (src/stream.rs:15)
/// Iterate over the items.
pub fn iter(&self) -> impl Iterator<Item = &u32>

Definition (src/stream.rs:14-17):
14-/// Iterate over the items.
15-pub fn iter(&self) -> impl Iterator<Item = &u32> {
16-    self.items.iter()
17-}

Implementations: none in the workspace.

---

fixture::Render (src/lib.rs:5)
/// Renders a value.
pub trait Render

Definition (src/lib.rs:4-8):
4-/// Renders a value.
5-pub trait Render {
6-    /// Render it.
7-    fn render(&self) -> String;
8-}

Implementations: 1 in 1 file, none in tests.
src/stream.rs (1)
in impl Render for Stream (20-25):
20:impl Render for Stream {
21-    fn render(&self) -> String {
22-        let total: u32 = self.iter().sum();
23-        format!(\"{total} {}\", self.config)
24-    }
25-}"
    );
}

/// `workspace/symbol` matches qualifiers against module paths rather than
/// `Self` types and includes `pub use` re-exports; verify that `Other::iter`
/// resolves through its `Self` type and `Stream` deduplicates its re-export.
#[tokio::test]
async fn resolves_names_through_their_type_and_past_re_exports() {
    let dir = fixture();
    let outcome = tools::run(
        ctx(dir.path()),
        tool(&json!({
            "targets": [{ "name": "Other::iter" }, { "name": "Stream" }],
            "show": ["definition"],
        })),
    )
    .await
    .unwrap();

    let Outcome::Success { content } = outcome else {
        panic!("{outcome:?}");
    };
    assert_eq!(
        content.as_str(),
        "\
fixture::Other::iter (src/lib.rs:13)
pub fn iter(&self)

Definition (src/lib.rs:13-13):
13-pub fn iter(&self) {}

---

fixture::stream::Stream (src/stream.rs:7)
/// A stream of numbers.
pub struct Stream {
    pub items: Vec<u32>,
    pub config: String,
}

Definition (src/stream.rs:3-11):
3-/// A stream of numbers.
4-///
5-/// Second paragraph.
6-#[derive(Debug, Default)]
7-pub struct Stream {
8-    /// The items.
9-    pub items: Vec<u32>,
10-    pub config: String,
11-}"
    );
}

/// Field references include shorthand initialization, struct literals,
/// assignments, destructuring, macro arguments (`format!`), and method bodies,
/// but exclude `macro_rules!` bodies and `#[derive]` expansions.
#[tokio::test]
async fn references_cover_what_the_description_says() {
    let dir = coverage_fixture();
    let outcome = tools::run(
        ctx(dir.path()),
        tool(&json!({
            "targets": [{ "name": "Point::x" }, { "name": "Derived::z" }],
        })),
    )
    .await
    .unwrap();

    let Outcome::Success { content } = outcome else {
        panic!("{outcome:?}");
    };
    // `via_macro` reads `x` through `get_x!` and is missing.
    assert_eq!(
        content.as_str(),
        "\
coverage::Point::x (src/lib.rs:2)
pub x: u32

References: 5 in 1 file, none in tests.
src/lib.rs (5)
in fn Point::new (7-9):
7-pub fn new(x: u32, y: u32) -> Self {
8:    Self { x, y }
9-}
in fn literal (14-16):
14-pub fn literal() -> Point {
15:    Point { x: 1, y: 2 }
16-}
in fn assign (18-21):
18-pub fn assign(p: &mut Point) {
19:    p.x = 3;
20-    p.y += 1;
21-}
in fn destructure (23-26):
23-pub fn destructure(p: &Point) -> u32 {
24:    let Point { x, .. } = p;
25-    *x
26-}
in fn <Point as Shape>::area (52-54):
52-fn area(&self) -> u32 {
53:    self.x * self.y
54-}

---

coverage::Derived::z (src/lib.rs:44)
pub z: u32

References: none in the workspace."
    );
}

/// Trait method calls resolve as references to the trait method, while
/// `implementations` lists each `impl` method body without duplicating it in
/// `references`.
#[tokio::test]
async fn a_trait_method_has_the_calls_and_its_implementations() {
    let dir = coverage_fixture();
    let outcome = tools::run(
        ctx(dir.path()),
        tool(&json!({
            "targets": [{ "name": "Shape::area" }],
            "show": ["implementations", "references"],
        })),
    )
    .await
    .unwrap();

    let Outcome::Success { content } = outcome else {
        panic!("{outcome:?}");
    };
    // The server also reports the impl's `fn area` as a reference; it shows
    // once, as the implementation.
    assert_eq!(
        content.as_str(),
        "\
coverage::Shape::area (src/lib.rs:48)
pub trait Shape
pub fn area(&self) -> u32

Implementations: 1 in 1 file, none in tests.
src/lib.rs (1)
in fn <Point as Shape>::area (52-54):
52:fn area(&self) -> u32 {
53-    self.x * self.y
54-}

References: 2 in 1 file, none in tests.
src/lib.rs (2)
in fn generic (57-59):
57-pub fn generic<T: Shape>(t: &T) -> u32 {
58:    t.area()
59-}
in fn dynamic (61-63):
61-pub fn dynamic(t: &dyn Shape) -> u32 {
62:    t.area()
63-}"
    );
}

#[tokio::test]
async fn members_count_each_field_and_method() {
    let dir = coverage_fixture();
    let outcome = tools::run(
        ctx(dir.path()),
        tool(&json!({
            "targets": [{ "name": "Point" }],
            "show": ["members"],
        })),
    )
    .await
    .unwrap();

    let Outcome::Success { content } = outcome else {
        panic!("{outcome:?}");
    };
    assert_eq!(
        content.as_str(),
        "\
coverage::Point (src/lib.rs:1)
pub struct Point {
    pub x: u32,
    pub y: u32,
}

Members: 4. References to each, outside tests and in tests:
field x (src/lib.rs:2): 5 outside tests, 0 in tests
  in fn Point::new (src/lib.rs:8)
  in fn literal (src/lib.rs:15)
  in fn assign (src/lib.rs:19)
  in fn destructure (src/lib.rs:24)
  in fn <Point as Shape>::area (src/lib.rs:53)
field y (src/lib.rs:3): 5 outside tests, 0 in tests
  in fn Point::new (src/lib.rs:8)
  in fn literal (src/lib.rs:15)
  in fn assign (src/lib.rs:20)
  in fn in_format (src/lib.rs:29)
  in fn <Point as Shape>::area (src/lib.rs:53)
fn new (src/lib.rs:7): no references
fn unused (src/lib.rs:11): no references"
    );
}

#[tokio::test]
async fn a_target_that_resolves_to_nothing_fails_the_call() {
    let dir = fixture();
    let outcome = tools::run(
        ctx(dir.path()),
        tool(&json!({ "targets": [{ "name": "nowhere" }] })),
    )
    .await
    .unwrap();

    let Outcome::Error { message, .. } = outcome else {
        panic!("{outcome:?}");
    };
    assert_eq!(
        message.as_str(),
        "nowhere: The workspace defines no item named `nowhere`. A field is found through its \
         type, as `Type::field`."
    );
}
