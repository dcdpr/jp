//! `code_symbol`: what a symbol is, where it is defined, what refers to it,
//! what implements it, and what it calls and is called by, as rust-analyzer
//! resolves them.
//! `code_rename`: rename a symbol everywhere it is used.
//!
//! Each call starts rust-analyzer in the workspace, waits for it to load,
//! answers with that one server, and shuts it down.
//! Nothing is kept between calls.
//!
//! [`lsp`] speaks the protocol, [`target`] turns a target into a position,
//! [`outline`] tells which item encloses a line, and [`render`] writes the
//! answer.
//! What holds only for rust-analyzer and Rust lives in [`rust_analyzer`]; the
//! rest asks the server standard LSP questions.

#[cfg(test)]
mod fixture;
mod lsp;
mod outline;
mod protocol;
mod render;
mod rust_analyzer;
mod target;

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fs, iter,
    rc::Rc,
    time::Duration,
};

use camino::{Utf8Path, Utf8PathBuf};
use fancy_regex::Regex;
use jp_tool::{AccessPolicy, Capability, Context, Outcome};
use serde::Deserialize;
use serde_json::{Value, json};
use url::Url;

use self::{
    lsp::{PositionEncoding, Server},
    outline::Item,
    protocol::{
        CallHierarchyItem, DocumentSymbol, GotoResponse, IncomingCall, Location, OutgoingCall,
        Position, Range, SelectionRange, SymbolInformation, TextEdit, WorkspaceEdit, hover_text,
        kind, kind_label,
    },
    render::{
        CALLER_LINES, Callee, Caller, Callers, Calls, FileHits, Group, GroupItem, Header, Hits,
        Member, Members, Report, Snippet,
    },
    target::Name,
};
use crate::{
    Tool,
    fs::{
        Change,
        utils::{authorize, resolve_workspace_path},
        write_changes,
    },
    util::{
        OneOrMany, ToolResult, error,
        root::{configured_root, resolve_root},
        unknown_tool,
    },
};

/// Lines the implementations and references sections take when one symbol is
/// asked about.
/// Several symbols share it evenly, down to [`MIN_REFERENCE_BUDGET`] each.
const REFERENCE_BUDGET: usize = 500;
const MIN_REFERENCE_BUDGET: usize = 60;

/// What each item one name matches gets of its symbol's budget before the rest
/// is shared by how many references each has.
const MIN_ITEM_BUDGET: usize = 20;

/// A name that matches up to this many items is answered for each of them; past
/// it, the candidates are listed.
const ANSWERED: usize = 3;

/// The most members listed, high enough that only a generated type reaches it.
const MEMBERS: usize = 1_000;

/// The most doc comment lines in a symbol's header.
const DOC_LINES: usize = 6;

/// The most lines below a reference that its statement is followed to.
const STATEMENT_LINES: u32 = 10;

/// The most candidates listed for an ambiguous name.
const CANDIDATES: usize = 15;

/// How many levels of callers `callers` lists, unless `depth` says otherwise.
const DEFAULT_CALLER_DEPTH: usize = 2;
const MAX_CALLER_DEPTH: usize = 5;

/// The most files a rename's summary lists by name.
const RENAMED_FILES: usize = 50;

/// What a rename leaves to check, ending its summary.
const RENAME_COVERAGE: &str = "rust-analyzer does not rename the name inside `macro_rules!` \
                               bodies, strings, comments, or non-Rust files. Run `cargo_check`, \
                               and search for the old name with `fs_grep_files` where those could \
                               hold it.";

const NOT_A_FUNCTION: &str = "the symbol is not a function or method.";

const DEFAULT_READY_TIMEOUT_SECS: u64 = 300;
const DEFAULT_REQUEST_TIMEOUT_SECS: u64 = 120;

#[expect(
    clippy::needless_pass_by_value,
    reason = "consistent with other module run fns"
)]
pub fn run(ctx: Context, t: Tool) -> ToolResult {
    match t.name.trim_start_matches("code_") {
        "symbol" => code_symbol(&ctx, &t),
        "rename" => code_rename(&ctx, &t),
        _ => unknown_tool(t),
    }
}

/// One symbol to look up.
#[derive(Debug, Deserialize)]
struct Target {
    name: String,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    line: Option<usize>,
    #[serde(default)]
    occurrence: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Show {
    Definition,
    References,
    Implementations,
    Members,
    Callers,
    Calls,
}

/// What `code_symbol` was asked to show for every target.
struct Query<'a> {
    show: &'a [Show],
    filters: &'a [Utf8PathBuf],

    /// How many levels of callers to list.
    depth: usize,

    /// Whether test code is shown in full.
    tests: bool,
}

fn code_symbol(ctx: &Context, t: &Tool) -> ToolResult {
    let targets = t.req::<OneOrMany<Target>>("targets")?.into_vec();
    if targets.is_empty() {
        return error("`targets` is empty. Name at least one symbol.");
    }

    let show = t
        .opt::<OneOrMany<Show>>("show")?
        .map_or_else(|| vec![Show::References], OneOrMany::into_vec);
    let tests = t.opt::<bool>("tests")?.unwrap_or(false);
    let depth = t
        .opt::<usize>("depth")?
        .unwrap_or(DEFAULT_CALLER_DEPTH)
        .clamp(1, MAX_CALLER_DEPTH);

    let mut filters = vec![];
    for path in t.opt::<OneOrMany<String>>("paths")?.unwrap_or_default() {
        match resolve_workspace_path(&ctx.root, &path, ctx.access.as_ref()) {
            Ok(resolved) => filters.push(resolved.absolute),
            Err(message) => return error(format!("`paths` entry `{path}`: {message}")),
        }
    }

    let pattern = match t.opt_or_empty::<String>("pattern")? {
        Some(pattern) => match Regex::new(&pattern) {
            Ok(pattern) => Some(pattern),
            Err(failure) => return error(format!("`pattern` is not a valid regex: {failure}")),
        },
        None => None,
    };

    let mut session = match open(ctx, t) {
        Ok(session) => session,
        Err(message) => return error(message),
    };
    session.pattern = pattern;
    let query = Query {
        show: &show,
        filters: &filters,
        depth,
        tests,
    };
    let budget = (REFERENCE_BUDGET / targets.len()).max(MIN_REFERENCE_BUDGET);

    let mut failures = 0;
    let blocks = targets
        .iter()
        .map(|target| match session.reports(target, &query) {
            Ok(reports) => {
                let weights = reports
                    .iter()
                    .map(|report| report.weight(tests))
                    .collect::<Vec<_>>();
                let rendered = reports
                    .iter()
                    .zip(render::shares(budget, &weights, MIN_ITEM_BUDGET))
                    .map(|(report, budget)| {
                        render::render(report, render::Options { budget, tests })
                    })
                    .collect::<Vec<_>>()
                    .join("\n\n---\n\n");

                if reports.len() > 1 {
                    format!(
                        "`{}` names {} items; each is answered below.\n\n{rendered}",
                        target.name,
                        reports.len()
                    )
                } else {
                    rendered
                }
            }
            Err(message) => {
                failures += 1;
                format!("{}: {message}", target.name)
            }
        })
        .collect::<Vec<_>>();

    session.server.shutdown();

    let text = blocks.join("\n\n---\n\n");

    if failures == targets.len() {
        return error(text);
    }

    Ok(text.into())
}

/// Start the language server in the workspace and wait for it to load it.
fn open<'a>(ctx: &'a Context, t: &Tool) -> Result<Session<'a>, String> {
    let configured = configured_root(&t.options)?;
    let root = resolve_root(
        &ctx.root,
        configured,
        ctx.access.as_ref(),
        rust_analyzer::CAPABILITIES,
        &rust_analyzer::ROOT,
    )?;

    let command = t.option_or("command", rust_analyzer::COMMAND.to_owned());
    let ready_timeout =
        Duration::from_secs(t.option_or("ready_timeout_secs", DEFAULT_READY_TIMEOUT_SECS));
    let request_timeout =
        Duration::from_secs(t.option_or("request_timeout_secs", DEFAULT_REQUEST_TIMEOUT_SECS));
    let mut settings = rust_analyzer::settings();
    if let Some(extra) = t.options.get("settings") {
        merge(&mut settings, extra);
    }

    let mut server = Server::start(
        &command,
        &[],
        &root,
        &settings,
        &rust_analyzer::experimental_capabilities(),
        request_timeout,
    )
    .map_err(|failure| failure.to_string())?;
    server
        .wait_until_ready(rust_analyzer::READY, ready_timeout)
        .map_err(|failure| failure.to_string())?;

    Ok(Session::new(server, &ctx.root, ctx.access.as_ref()))
}

/// One rename `code_rename` was asked for: the item, named the way a
/// `code_symbol` target is, and its new name.
#[derive(Debug, Deserialize)]
struct RenameRequest {
    name: String,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    line: Option<usize>,
    #[serde(default)]
    occurrence: Option<usize>,
    new_name: String,
}

impl RenameRequest {
    fn target(&self) -> Target {
        Target {
            name: self.name.clone(),
            path: self.path.clone(),
            line: self.line,
            occurrence: self.occurrence,
        }
    }
}

fn code_rename(ctx: &Context, t: &Tool) -> ToolResult {
    let mut requests = t.req::<OneOrMany<RenameRequest>>("renames")?.into_vec();
    for request in &mut requests {
        request.new_name = request.new_name.trim().to_owned();
    }
    if let Err(message) = check_renames(&requests) {
        return error(message);
    }

    if ctx.action.is_format_arguments() {
        let renames = requests
            .iter()
            .map(|request| format!("`{}` to `{}`", request.name, request.new_name))
            .collect::<Vec<_>>()
            .join(", ");
        return Ok(format!("Rename {renames}.").into());
    }

    let mut session = match open(ctx, t) {
        Ok(session) => session,
        Err(message) => return error(message),
    };
    let renamed = session.rename(&requests);
    session.server.shutdown();

    let renamed = match renamed {
        Ok(renamed) => renamed,
        Err(message) => return error(message),
    };
    if renamed.changes.is_empty() {
        return error("The renames leave every file as it was.");
    }

    let summary = rename_summary(&renamed);

    // The patch the approval shows is for the user; the assistant gets the
    // summary, which does not grow with the number of edits.
    match write_changes(renamed.changes, ctx, &t.answers, &t.options)? {
        Outcome::Success { .. } => Ok(summary.into()),
        other => Ok(other),
    }
}

/// Refuse a batch of renames before any server starts: an empty one, an empty
/// new name, and a batch whose result would depend on its order.
///
/// The order matters when two renames give the same name, or when one gives a
/// name that another takes away, as a swap does.
fn check_renames(requests: &[RenameRequest]) -> Result<(), String> {
    if requests.is_empty() {
        return Err("`renames` is empty. Give at least one rename.".to_owned());
    }

    let mut new_names = HashSet::new();
    for request in requests {
        if request.new_name.is_empty() {
            return Err(format!(
                "`new_name` for `{}` is empty. Give the new identifier.",
                request.name
            ));
        }
        if !new_names.insert(request.new_name.as_str()) {
            return Err(format!(
                "Two renames give the name `{}`. Rename them in separate calls.",
                request.new_name
            ));
        }
    }

    for (index, request) in requests.iter().enumerate() {
        let old = Name::parse(&request.name)?;
        let taker = requests
            .iter()
            .enumerate()
            .find(|&(other, taker)| other != index && taker.new_name == old.bare_ident());
        if let Some((_, taker)) = taker {
            return Err(format!(
                "Renaming `{}` gives it the name `{}`, which renaming `{}` takes away; the result \
                 would depend on the order. Rename them in separate calls.",
                taker.name, taker.new_name, request.name
            ));
        }
    }

    Ok(())
}

/// `message` about rename `index`, naming the rename when there are several.
fn in_rename(requests: &[RenameRequest], index: usize, message: String) -> String {
    if requests.len() > 1 {
        format!(
            "Rename {} (`{}`): {message}",
            index + 1,
            requests[index].name
        )
    } else {
        message
    }
}

/// Renames worked out but not yet written.
struct Renamed {
    renames: Vec<RenameOutcome>,

    /// Each edited file with its number of edits across every rename.
    files: BTreeMap<String, usize>,

    changes: Vec<Change>,
}

struct RenameOutcome {
    /// The name as the request gave it: `Stream::iter`.
    name: String,

    /// Where the renamed item is defined: `src/stream.rs:15`.
    location: String,

    new_name: String,
    edits: usize,
    files: usize,
}

fn rename_summary(renamed: &Renamed) -> String {
    let mut out = renamed
        .renames
        .iter()
        .map(|rename| {
            let edits = if rename.edits == 1 { "edit" } else { "edits" };
            let files = if rename.files == 1 { "file" } else { "files" };
            format!(
                "Renamed `{}` ({}) to `{}`: {} {edits} in {} {files}.",
                rename.name, rename.location, rename.new_name, rename.edits, rename.files
            )
        })
        .collect::<Vec<_>>();
    out.extend(
        renamed
            .files
            .iter()
            .take(RENAMED_FILES)
            .map(|(path, count)| format!("{path} ({count})")),
    );
    if renamed.files.len() > RENAMED_FILES {
        out.push(format!(
            "and {} more files",
            renamed.files.len() - RENAMED_FILES
        ));
    }
    out.push(RENAME_COVERAGE.to_owned());

    out.join("\n")
}

/// Merge `extra` into `base`, object keys recursively and anything else by
/// replacement.
fn merge(base: &mut Value, extra: &Value) {
    match (base, extra) {
        (Value::Object(base), Value::Object(extra)) => {
            for (key, value) in extra {
                merge(base.entry(key.clone()).or_insert(Value::Null), value);
            }
        }
        (base, extra) => *base = extra.clone(),
    }
}

/// A rename with its target found, before any rename is made.
struct Planned {
    name: Name,
    new_name: String,

    /// Where the item is defined: `src/stream.rs:15`, from before any rename.
    location: String,

    /// The file the item is defined in, relative to the workspace root.
    path: String,
    file: Utf8PathBuf,

    /// Where the item is defined, in the file as the earlier renames left it.
    position: Position,
}

/// A file as it was and as the renames so far left it.
struct Edited {
    relative: Utf8PathBuf,
    before: String,
    current: String,
}

/// A file's text and items, read once per call.
struct Source {
    lines: Rc<[String]>,
    items: Vec<Item>,
}

/// The server and what has been read from the workspace during one call.
struct Session<'a> {
    server: Server,
    root: &'a Utf8Path,

    /// `root` with symlinks resolved, since the server reports canonical paths.
    canonical_root: Utf8PathBuf,

    access: Option<&'a AccessPolicy>,
    sources: HashMap<Utf8PathBuf, Rc<Source>>,

    /// The version last sent for each file the server was given the text of.
    versions: HashMap<Utf8PathBuf, i32>,

    /// Narrows the references and implementations shown to the items with a
    /// line it matches.
    pattern: Option<Regex>,
}

impl<'a> Session<'a> {
    fn new(server: Server, root: &'a Utf8Path, access: Option<&'a AccessPolicy>) -> Self {
        Self {
            server,
            root,
            canonical_root: root.canonicalize_utf8().unwrap_or_else(|_| root.to_owned()),
            access,
            sources: HashMap::new(),
            versions: HashMap::new(),
            pattern: None,
        }
    }

    /// One report per item the target names, up to [`ANSWERED`] of them.
    fn reports(&mut self, target: &Target, query: &Query<'_>) -> Result<Vec<Report>, String> {
        let name = Name::parse(&target.name)?;

        self.definitions(target, &name)?
            .into_iter()
            .map(|(file, position)| self.report(&file, position, &name, query))
            .collect()
    }

    /// Where each item the target names is defined, without repeats.
    fn definitions(
        &mut self,
        target: &Target,
        name: &Name,
    ) -> Result<Vec<(Utf8PathBuf, Position)>, String> {
        let mut definitions: Vec<(Utf8PathBuf, Position)> = vec![];
        for (file, position) in self.locate(target, name)? {
            let found = self
                .definition(&file, position)?
                .unwrap_or((file, position));
            if !definitions.contains(&found) {
                definitions.push(found);
            }
        }

        Ok(definitions)
    }

    fn report(
        &mut self,
        file: &Utf8Path,
        position: Position,
        name: &Name,
        query: &Query<'_>,
    ) -> Result<Report, String> {
        let show = query.show;
        let header = self.header(file, position, name)?;
        let definition = if show.contains(&Show::Definition) {
            self.snippet(file, position)?
        } else {
            None
        };
        let implementations = if show.contains(&Show::Implementations) {
            Some(self.implementations(file, position, query.filters)?)
        } else {
            None
        };
        let references = if show.contains(&Show::References) {
            Some(self.references(file, position, query.filters, true)?)
        } else {
            None
        };
        let callers = if show.contains(&Show::Callers) {
            Some(self.callers(file, position, query.depth, query.tests)?)
        } else {
            None
        };
        let calls = if show.contains(&Show::Calls) {
            Some(self.calls(file, position)?)
        } else {
            None
        };
        let members = if show.contains(&Show::Members) {
            Some(self.members(file, position, query.filters)?)
        } else {
            None
        };

        Ok(Report {
            header,
            definition,
            implementations,
            references,
            callers,
            calls,
            members,
        })
    }

    /// The call hierarchy item for the function defined at `position`, or
    /// `None` when it is not a function.
    fn call_item(
        &mut self,
        file: &Utf8Path,
        position: Position,
    ) -> Result<Option<CallHierarchyItem>, String> {
        Ok(self
            .server
            .request::<Option<Vec<CallHierarchyItem>>>(
                "textDocument/prepareCallHierarchy",
                &position_params(file, position)?,
            )
            .map_err(|failure| failure.to_string())?
            .and_then(|items| items.into_iter().next()))
    }

    /// The functions that call the function at `position`, to `depth` levels.
    ///
    /// Test functions are counted rather than listed unless `tests` asks for
    /// them, and their own callers are never followed.
    fn callers(
        &mut self,
        file: &Utf8Path,
        position: Position,
        depth: usize,
        tests: bool,
    ) -> Result<Callers, String> {
        let Some(item) = self.call_item(file, position)? else {
            return Ok(Callers {
                note: Some(NOT_A_FUNCTION.to_owned()),
                ..Callers::default()
            });
        };

        let mut callers = Callers {
            depth,
            ..Callers::default()
        };
        let mut seen = HashSet::from([(item.uri.clone(), item.selection_range.start)]);
        self.walk_callers(&item, 0, depth, tests, &mut seen, &mut callers)?;

        Ok(callers)
    }

    /// List the callers of `item` at `level`, each followed by its own callers
    /// while `level` is above `depth`.
    ///
    /// A function already listed is listed again without its callers, so a
    /// recursive call ends the walk.
    fn walk_callers(
        &mut self,
        item: &CallHierarchyItem,
        level: usize,
        depth: usize,
        tests: bool,
        seen: &mut HashSet<(Url, Position)>,
        callers: &mut Callers,
    ) -> Result<(), String> {
        let incoming = self
            .server
            .request::<Option<Vec<IncomingCall>>>(
                "callHierarchy/incomingCalls",
                &json!({ "item": item }),
            )
            .map_err(|failure| failure.to_string())?
            .unwrap_or_default();

        let mut found = vec![];
        for call in incoming {
            let Ok(file) = file_path(&call.from.uri) else {
                continue;
            };
            let line = call
                .from_ranges
                .iter()
                .map(|range| range.start.line)
                .min()
                .unwrap_or(call.from.selection_range.start.line);
            let (label, test) = self
                .label_at(&file, call.from.selection_range.start)
                .unwrap_or_else(|| {
                    let test = self
                        .relative(&file)
                        .is_some_and(|path| rust_analyzer::is_test_path(&path));
                    (format!("fn {}", call.from.name), test)
                });
            found.push((test, file, line, label, call));
        }
        // Callers outside tests first, then in file and line order.
        found.sort_by(|a, b| (a.0, &a.1, a.2).cmp(&(b.0, &b.1, b.2)));

        for (test, file, line, label, call) in found {
            if test && !tests {
                callers.hidden_tests += 1;
                continue;
            }
            if callers.shown.len() >= CALLER_LINES {
                callers.stopped = true;
                return Ok(());
            }

            let repeated = !seen.insert((call.from.uri.clone(), call.from.selection_range.start));
            callers.shown.push(Caller {
                label,
                location: self.display(&file, line),
                calls: call.from_ranges.len(),
                level,
                repeated,
            });

            if !repeated && !test && level + 1 < depth {
                self.walk_callers(&call.from, level + 1, depth, tests, seen, callers)?;
            }
        }

        Ok(())
    }

    /// The functions the function at `position` calls, in the order of their
    /// first call.
    fn calls(&mut self, file: &Utf8Path, position: Position) -> Result<Calls, String> {
        let Some(item) = self.call_item(file, position)? else {
            return Ok(Calls {
                note: Some(NOT_A_FUNCTION.to_owned()),
                ..Calls::default()
            });
        };

        let mut outgoing = self
            .server
            .request::<Option<Vec<OutgoingCall>>>(
                "callHierarchy/outgoingCalls",
                &json!({ "item": item }),
            )
            .map_err(|failure| failure.to_string())?
            .unwrap_or_default();
        outgoing.sort_by_key(|call| call.from_ranges.iter().map(|range| range.start).min());

        let mut calls = Calls::default();
        for call in outgoing {
            let Ok(callee) = file_path(&call.to.uri) else {
                continue;
            };
            // The server lists the structs and variants a function constructs
            // beside its calls. Outside the workspace those are `Some`, `Ok`
            // and the like, which say nothing.
            let function = is_function_kind(call.to.kind);
            if self.relative(&callee).is_none() {
                if function && !calls.outside.contains(&call.to.name) {
                    calls.outside.push(call.to.name.clone());
                }
                continue;
            }

            let label = match self.item_at(&callee, call.to.selection_range.start) {
                // The server places a method a derive generates on the type the
                // derive sits on.
                Some(item) if function && !is_function_kind(item.kind) => format!(
                    "derived fn {}{}{}",
                    item.name,
                    rust_analyzer::PATH_SEPARATOR,
                    call.to.name
                ),
                Some(item) => item_label(&item),
                None => format!("{} {}", kind_label(call.to.kind), call.to.name),
            };
            if !function {
                if !calls.named.contains(&label) {
                    calls.named.push(label);
                }
                continue;
            }

            let mut lines = call
                .from_ranges
                .iter()
                .map(|range| to_usize(range.start.line) + 1)
                .collect::<Vec<_>>();
            lines.sort_unstable();
            lines.dedup();

            calls.inside.push(Callee {
                label,
                location: self.display(&callee, call.to.selection_range.start.line),
                lines,
            });
        }

        Ok(calls)
    }

    /// The label of the item defined at `position`, and whether it is test
    /// code, when the tool may read its file.
    fn label_at(&mut self, file: &Utf8Path, position: Position) -> Option<(String, bool)> {
        let item = self.item_at(file, position)?;
        let test_path = self
            .relative(file)
            .is_some_and(|path| rust_analyzer::is_test_path(&path));

        Some((
            item_label(&item),
            test_path || item.in_tests || is_test_module(&item),
        ))
    }

    /// The item defined at `position`, when the tool may read its file.
    fn item_at(&mut self, file: &Utf8Path, position: Position) -> Option<Item> {
        let path = self.relative(file)?;
        self.readable(&path).ok()?;
        let source = self.source(file).ok()?;

        outline::at(&source.items, position).map(|index| source.items[index].clone())
    }

    /// Where each rename's item is defined, found in the files as they are
    /// before any rename.
    fn plan_renames(&mut self, requests: &[RenameRequest]) -> Result<Vec<Planned>, String> {
        let context = |index: usize, message: String| in_rename(requests, index, message);

        let mut planned: Vec<Planned> = vec![];
        for (index, request) in requests.iter().enumerate() {
            let (name, file, position) = self
                .rename_target(&request.target())
                .map_err(|message| context(index, message))?;
            let path = self
                .relative(&file)
                .ok_or_else(|| context(index, format!("`{file}` is outside the workspace.")))?;
            if let Some(other) = planned
                .iter()
                .find(|other| other.path == path && other.position == position)
            {
                return Err(format!(
                    "`{}` and `{}` name the same item. Rename it once.",
                    other.name.query(),
                    name.query()
                ));
            }

            planned.push(Planned {
                location: self.display(&file, position.line),
                name,
                new_name: request.new_name.clone(),
                path,
                file,
                position,
            });
        }

        Ok(planned)
    }

    /// The edits that make every rename in `requests`, in order, as the changed
    /// contents of each file.
    ///
    /// Every target is found in the files as they are before any rename.
    /// Each rename after the first is worked out against the text the earlier
    /// ones left, which the server is sent, and a target an earlier rename
    /// moved is followed to its new place.
    /// Any failure leaves every file as it was.
    fn rename(&mut self, requests: &[RenameRequest]) -> Result<Renamed, String> {
        let context = |index: usize, message: String| in_rename(requests, index, message);
        let mut planned = self.plan_renames(requests)?;

        let encoding = self.server.encoding();
        let mut files: BTreeMap<String, Edited> = BTreeMap::new();
        let mut renamed = Renamed {
            renames: vec![],
            files: BTreeMap::new(),
            changes: vec![],
        };

        for index in 0..planned.len() {
            let rename = &planned[index];
            let (file, position) = (rename.file.clone(), rename.position);
            let (name, new_name) = (rename.name.clone(), rename.new_name.clone());
            let edits = self
                .rename_edits(&file, position, &name, &new_name)
                .map_err(|message| context(index, message))?;
            if edits.is_empty() {
                return Err(context(
                    index,
                    format!("rust-analyzer made no edits to rename `{}`.", name.query()),
                ));
            }

            let mut count = 0;
            for (uri, edits) in &edits {
                let edited = file_path(uri)?;
                let path = self.relative(&edited).ok_or_else(|| {
                    context(
                        index,
                        format!("the rename would edit `{edited}`, outside the workspace."),
                    )
                })?;
                if !files.contains_key(&path) {
                    let resolved = resolve_workspace_path(self.root, &path, self.access)?;
                    authorize(self.access, Capability::Update, &resolved)?;
                    let text = fs::read_to_string(&resolved.absolute)
                        .map_err(|error| format!("Could not read `{path}`: {error}"))?;
                    files.insert(path.clone(), Edited {
                        relative: resolved.relative,
                        before: text.clone(),
                        current: text,
                    });
                }
                let Some(entry) = files.get_mut(&path) else {
                    continue;
                };

                let spans = edit_spans(&entry.current, edits, encoding, name.bare_ident())
                    .map_err(|message| context(index, format!("`{path}`: {message}")))?;
                let after = splice(&entry.current, &spans);
                for later in planned[index + 1..]
                    .iter_mut()
                    .filter(|later| later.path == path)
                {
                    later.position =
                        shift_position(&entry.current, &after, &spans, later.position, encoding)
                            .ok_or_else(|| {
                                format!(
                                    "Renaming `{}` rewrites where `{}` is defined. Rename them in \
                                     separate calls.",
                                    name.query(),
                                    later.name.query()
                                )
                            })?;
                }
                entry.current = after;

                count += edits.len();
                *renamed.files.entry(path).or_default() += edits.len();
                if index + 1 < planned.len() {
                    let text = entry.current.clone();
                    self.sync(&edited, &text)?;
                }
            }

            renamed.renames.push(RenameOutcome {
                name: name.query(),
                location: planned[index].location.clone(),
                new_name,
                edits: count,
                files: edits.len(),
            });
        }

        renamed.changes = files
            .into_values()
            .filter(|edited| edited.before != edited.current)
            .map(|edited| Change {
                path: edited.relative,
                before: edited.before,
                after: edited.current,
            })
            .collect();

        Ok(renamed)
    }

    /// The one item `target` names, for renaming.
    fn rename_target(&mut self, target: &Target) -> Result<(Name, Utf8PathBuf, Position), String> {
        let name = Name::parse(&target.name)?;
        let definitions = self.definitions(target, &name)?;
        let [(file, position)] = <[_; 1]>::try_from(definitions).map_err(|definitions| {
            format!(
                "`{}` names {} items. Rename one at a time: give `path` and `line`:\n{}",
                name.query(),
                definitions.len(),
                definitions
                    .iter()
                    .map(|(file, position)| format!("- {}", self.display(file, position.line)))
                    .collect::<Vec<_>>()
                    .join("\n")
            )
        })?;

        Ok((name, file, position))
    }

    /// The server's edits for renaming the item at `position` to `new_name`,
    /// per file.
    fn rename_edits(
        &mut self,
        file: &Utf8Path,
        position: Position,
        name: &Name,
        new_name: &str,
    ) -> Result<Vec<(Url, Vec<TextEdit>)>, String> {
        let mut params = position_params(file, position)?;
        params["newName"] = json!(new_name);
        let edit = self
            .server
            .request::<Option<WorkspaceEdit>>("textDocument/rename", &params)
            .map_err(|failure| failure.to_string())?
            .ok_or_else(|| format!("rust-analyzer cannot rename `{}`.", name.query()))?;

        edit.into_text_edits()
            .map_err(|message| format!("Renaming `{}` was refused: {message}.", name.query()))
    }

    /// Give the server `text` as the contents of `file`, which it then answers
    /// from instead of the file on disk.
    fn sync(&mut self, file: &Utf8Path, text: &str) -> Result<(), String> {
        let uri = uri(file)?;
        let version = {
            let version = self.versions.entry(file.to_owned()).or_insert(0);
            *version += 1;
            *version
        };

        let (method, params) = if version == 1 {
            (
                "textDocument/didOpen",
                json!({
                    "textDocument": {
                        "uri": uri,
                        "languageId": rust_analyzer::LANGUAGE_ID,
                        "version": version,
                        "text": text,
                    },
                }),
            )
        } else {
            (
                "textDocument/didChange",
                json!({
                    "textDocument": { "uri": uri, "version": version },
                    "contentChanges": [{ "text": text }],
                }),
            )
        };

        self.server
            .notify(method, &params)
            .map_err(|failure| failure.to_string())
    }

    /// The files and positions a target names: one, or up to [`ANSWERED`] when
    /// a name matches several items.
    fn locate(
        &mut self,
        target: &Target,
        name: &Name,
    ) -> Result<Vec<(Utf8PathBuf, Position)>, String> {
        match (&target.path, target.line) {
            (None, Some(_)) => Err("`line` needs `path`.".to_owned()),
            (None, None) => self.search(name),
            (Some(path), Some(line)) => {
                let file = self.readable(path)?;
                let source = self.source(&file)?;
                let text = line
                    .checked_sub(1)
                    .and_then(|index| source.lines.get(index))
                    .ok_or_else(|| {
                        format!(
                            "`{path}` has {} lines; there is no line {line}.",
                            source.lines.len()
                        )
                    })?;
                let byte =
                    target::pick(text, &name.ident, target.occurrence).map_err(|message| {
                        format!("{message} Line {line} of `{path}`: `{}`", text.trim())
                    })?;

                Ok(vec![(file, Position {
                    line: to_u32(line - 1),
                    character: self.server.encoding().character(text, byte),
                })])
            }
            (Some(path), None) => {
                let file = self.readable(path)?;
                let source = self.source(&file)?;
                match outline::matching(&source.items, &name.qualifier, &name.ident).as_slice() {
                    [] => Err(format!(
                        "`{path}` defines no item named `{}`.",
                        name.query()
                    )),
                    few if few.len() <= ANSWERED => Ok(few
                        .iter()
                        .map(|&index| (file.clone(), source.items[index].selection.start))
                        .collect()),
                    many => Err(format!(
                        "`{}` names {} items in `{path}`. Qualify it, or give `line`:\n{}",
                        name.query(),
                        many.len(),
                        many.iter()
                            .map(|&index| {
                                let item = &source.items[index];
                                format!(
                                    "- {} {} (line {})",
                                    kind_label(item.kind),
                                    item.name,
                                    item.selection.start.line + 1
                                )
                            })
                            .collect::<Vec<_>>()
                            .join("\n")
                    )),
                }
            }
        }
    }

    /// Find a name with the server's workspace symbol search.
    ///
    /// The search is asked for the last segment only, since it matches a
    /// qualifier against module paths and not against the type a method is
    /// implemented on.
    /// The qualifier then picks among the exact matches, and a `use` that
    /// re-exports a match is dropped in favor of the definition.
    ///
    /// The search does not index fields, so a qualified name that matches
    /// nothing is looked up as a member of its qualifier.
    fn search(&mut self, name: &Name) -> Result<Vec<(Utf8PathBuf, Position)>, String> {
        let symbols = self
            .server
            .request::<Option<Vec<SymbolInformation>>>(
                "workspace/symbol",
                &json!({ "query": name.bare_ident() }),
            )
            .map_err(|failure| failure.to_string())?
            .unwrap_or_default();
        let saturated = symbols.len() >= rust_analyzer::SYMBOL_LIMIT;

        let named = target::exact(symbols, name.bare_ident());
        let fitting = named
            .iter()
            .filter(|symbol| self.fits(symbol, &name.qualifier))
            .cloned()
            .collect::<Vec<_>>();
        let found = self.without_re_exports(fitting)?;

        match found.as_slice() {
            [] => self
                .not_found(name, &named, saturated)
                .map(|found| vec![found]),
            few if few.len() <= ANSWERED => few
                .iter()
                .map(|symbol| {
                    Ok((
                        file_path(&symbol.location.uri)?,
                        symbol.location.range.start,
                    ))
                })
                .collect(),
            many => Err(format!(
                "`{}` names {} items. Qualify it with its type or module, or give `path` and \
                 `line`:\n{}",
                name.query(),
                many.len(),
                self.candidates(many)
            )),
        }
    }

    /// Why a name matched nothing, or the member it names when its qualifier is
    /// a type with that field.
    fn not_found(
        &mut self,
        name: &Name,
        named: &[SymbolInformation],
        saturated: bool,
    ) -> Result<(Utf8PathBuf, Position), String> {
        let member = name.parent().map(|parent| self.member(&parent, name));
        if let Some(Ok(found)) = member {
            return Ok(found);
        }

        if !named.is_empty() {
            return Err(format!(
                "No item named `{}` is in `{}`. The items named `{}` are:\n{}",
                name.ident,
                name.qualifier.join(rust_analyzer::PATH_SEPARATOR),
                name.ident,
                self.candidates(named)
            ));
        }

        if saturated {
            return Err(format!(
                "The symbol search stopped at {} fuzzy matches for `{}` before finding it. Give \
                 `path`, or `path` and `line`.",
                rust_analyzer::SYMBOL_LIMIT,
                name.ident
            ));
        }

        match member {
            Some(Err(message)) => Err(message),
            _ => Err(format!(
                "The workspace defines no item named `{}`. A field is found through its type, as \
                 `Type::field`.",
                name.ident
            )),
        }
    }

    /// Whether a symbol sits where `qualifier` says, judged by its file and the
    /// type or trait it is defined on.
    fn fits(&self, symbol: &SymbolInformation, qualifier: &[String]) -> bool {
        if qualifier.is_empty() {
            return true;
        }

        let path = file_path(&symbol.location.uri)
            .ok()
            .and_then(|file| self.relative(&file))
            .unwrap_or_default();

        target::qualifier_fits(qualifier, &path, symbol.container_name.as_deref())
    }

    /// `symbols` without the ones that only re-export an item: a `pub use` is
    /// reported under the name it brings in, alongside the definition.
    ///
    /// A symbol in a file the tool may not read is kept, since it cannot be
    /// told apart.
    /// If every symbol is a re-export, all are kept.
    fn without_re_exports(
        &mut self,
        symbols: Vec<SymbolInformation>,
    ) -> Result<Vec<SymbolInformation>, String> {
        if symbols.len() < 2 {
            return Ok(symbols);
        }

        let mut definitions = vec![];
        for symbol in &symbols {
            let readable = file_path(&symbol.location.uri).ok().filter(|file| {
                self.relative(file)
                    .is_some_and(|path| self.readable(&path).is_ok())
            });
            let defines = match readable {
                Some(file) => {
                    outline::defines(&self.source(&file)?.items, symbol.location.range.start)
                }
                None => true,
            };

            if defines {
                definitions.push(symbol.clone());
            }
        }

        Ok(if definitions.is_empty() {
            symbols
        } else {
            definitions
        })
    }

    /// A list of symbols for an error, up to [`CANDIDATES`] of them.
    fn candidates(&self, symbols: &[SymbolInformation]) -> String {
        let mut listed = symbols
            .iter()
            .take(CANDIDATES)
            .map(|symbol| self.candidate(symbol))
            .collect::<Vec<_>>();
        if symbols.len() > CANDIDATES {
            listed.push(format!("- ... and {} more", symbols.len() - CANDIDATES));
        }

        listed.join("\n")
    }

    /// Find `name` among the members of the item `parent` names.
    fn member(&mut self, parent: &Name, name: &Name) -> Result<(Utf8PathBuf, Position), String> {
        let parents = self.search(parent).map_err(|message| {
            format!(
                "The workspace defines no item named `{}`, and its qualifier did not resolve: \
                 {message}",
                name.query()
            )
        })?;
        let [(file, position)] = <[_; 1]>::try_from(parents).map_err(|parents| {
            format!(
                "The workspace defines no item named `{}`, and its qualifier `{}` names {} items. \
                 Qualify it further.",
                name.query(),
                parent.query(),
                parents.len()
            )
        })?;

        let source = self.source(&file)?;
        let found = outline::at(&source.items, position)
            .map(|index| outline::children_named(&source.items, index, &name.ident))
            .unwrap_or_default();

        match found.as_slice() {
            [only] => Ok((file, source.items[*only].selection.start)),
            _ => Err(format!(
                "The workspace defines no item named `{}`, and `{}` has no member named `{}`.",
                name.query(),
                parent.query(),
                name.ident
            )),
        }
    }

    fn candidate(&self, symbol: &SymbolInformation) -> String {
        let name = match &symbol.container_name {
            Some(container) => format!("{container}::{}", symbol.name),
            None => symbol.name.clone(),
        };
        let location = file_path(&symbol.location.uri).map_or_else(
            |_| symbol.location.uri.to_string(),
            |path| self.display(&path, symbol.location.range.start.line),
        );

        format!("- {} {name} ({location})", kind_label(symbol.kind))
    }

    /// Where the symbol at `position` is defined, if the server knows.
    fn definition(
        &mut self,
        file: &Utf8Path,
        position: Position,
    ) -> Result<Option<(Utf8PathBuf, Position)>, String> {
        let found = self
            .server
            .request::<Option<GotoResponse>>(
                "textDocument/definition",
                &position_params(file, position)?,
            )
            .map_err(|failure| failure.to_string())?;

        Ok(found
            .map(GotoResponse::into_locations)
            .unwrap_or_default()
            .into_iter()
            .find_map(|location| Some((file_path(&location.uri).ok()?, location.range.start))))
    }

    fn header(
        &mut self,
        file: &Utf8Path,
        position: Position,
        name: &Name,
    ) -> Result<Header, String> {
        let hover = self
            .server
            .request::<Value>("textDocument/hover", &position_params(file, position)?)
            .map_err(|failure| failure.to_string())?;
        let parts = hover_text(&hover)
            .map(|text| rust_analyzer::parse_hover(&text, render::SIGNATURE_LINES))
            .unwrap_or_default();

        let qualified = match parts.container {
            Some(container) => {
                format!("{container}{}{}", rust_analyzer::PATH_SEPARATOR, name.ident)
            }
            None => name.ident.clone(),
        };

        Ok(Header {
            qualified,
            location: self.display(file, position.line),
            signature: parts.signature,
            doc: self.doc(file, position)?,
        })
    }

    /// The first doc paragraph of the item defined at `position`, when the tool
    /// may read its file.
    fn doc(&mut self, file: &Utf8Path, position: Position) -> Result<Vec<String>, String> {
        let readable = self
            .relative(file)
            .is_some_and(|path| self.readable(&path).is_ok());
        if !readable {
            return Ok(vec![]);
        }

        let source = self.source(file)?;
        Ok(outline::at(&source.items, position)
            .map(|index| outline::doc_paragraph(&source.items[index], &source.lines, DOC_LINES))
            .unwrap_or_default())
    }

    /// The source of the item defined at `position`, when it is in the
    /// workspace.
    fn snippet(&mut self, file: &Utf8Path, position: Position) -> Result<Option<Snippet>, String> {
        let Some(path) = self.relative(file) else {
            return Ok(None);
        };
        if self.readable(&path).is_err() {
            return Ok(None);
        }

        let source = self.source(file)?;
        let Some(index) = outline::at(&source.items, position) else {
            return Ok(None);
        };

        let item = &source.items[index];
        let start = to_usize(item.range.start.line);
        let end = to_usize(item.range.end.line).min(source.lines.len().saturating_sub(1));

        Ok(source.lines.get(start..=end).map(|lines| Snippet {
            path,
            start,
            lines: lines.to_vec(),
        }))
    }

    /// What implements the symbol at `position`: the `impl` blocks of a trait
    /// or type, or each implementation of a trait method.
    fn implementations(
        &mut self,
        file: &Utf8Path,
        position: Position,
        filters: &[Utf8PathBuf],
    ) -> Result<Hits, String> {
        let locations = self.implementation_locations(file, position)?;
        self.hits(locations, filters, true)
    }

    fn implementation_locations(
        &mut self,
        file: &Utf8Path,
        position: Position,
    ) -> Result<Vec<Location>, String> {
        Ok(self
            .server
            .request::<Option<GotoResponse>>(
                "textDocument/implementation",
                &position_params(file, position)?,
            )
            .map_err(|failure| failure.to_string())?
            .map(GotoResponse::into_locations)
            .unwrap_or_default())
    }

    /// Every use of the symbol at `position`.
    ///
    /// The server counts each implementation of a trait method as a reference
    /// to it; those are left out, so a trait method's references are its calls.
    /// With `statements`, each reference is followed to the end of its
    /// statement for showing.
    fn references(
        &mut self,
        file: &Utf8Path,
        position: Position,
        filters: &[Utf8PathBuf],
        statements: bool,
    ) -> Result<Hits, String> {
        let mut params = position_params(file, position)?;
        params["context"] = json!({ "includeDeclaration": false });

        let mut locations = self
            .server
            .request::<Option<Vec<Location>>>("textDocument/references", &params)
            .map_err(|failure| failure.to_string())?
            .unwrap_or_default();

        if self.is_function(file, position)? {
            let implementations = self.implementation_locations(file, position)?;
            locations.retain(|location| {
                !implementations.iter().any(|implementation| {
                    implementation.uri == location.uri
                        && implementation.range.start == location.range.start
                })
            });
        }

        self.hits(locations, filters, statements)
    }

    /// Whether the item defined at `position` is a function or method, when the
    /// tool may read its file.
    fn is_function(&mut self, file: &Utf8Path, position: Position) -> Result<bool, String> {
        let readable = self
            .relative(file)
            .is_some_and(|path| self.readable(&path).is_ok());
        if !readable {
            return Ok(false);
        }

        let source = self.source(file)?;
        Ok(outline::at(&source.items, position)
            .is_some_and(|index| matches!(source.items[index].kind, kind::METHOD | kind::FUNCTION)))
    }

    /// Locations grouped by file and enclosing item, counted across the
    /// workspace and kept for showing under `filters`.
    ///
    /// With `statements`, each shown location is followed to the end of the
    /// statement it starts.
    fn hits(
        &mut self,
        locations: Vec<Location>,
        filters: &[Utf8PathBuf],
        statements: bool,
    ) -> Result<Hits, String> {
        let pattern = self.pattern.clone().filter(|_| statements);
        let mut hits = Hits {
            filtered: !filters.is_empty(),
            pattern: pattern.as_ref().map(|pattern| pattern.as_str().to_owned()),
            ..Hits::default()
        };

        for (file, positions) in by_file(locations) {
            let lines = positions
                .iter()
                .map(|position| position.line)
                .collect::<Vec<_>>();
            let Some(path) = self.relative(&file) else {
                hits.outside_workspace += lines.len();
                continue;
            };

            hits.total += lines.len();
            hits.files += 1;

            if self.readable(&path).is_err() {
                hits.withheld += lines.len();
                continue;
            }

            // Grouped before the `paths` filter applies, so the split between
            // test code and the rest covers the whole workspace.
            let source = self.source(&file)?;
            let mut groups = group_lines(
                &source.items,
                &source.lines,
                &lines,
                rust_analyzer::is_test_path(&path),
            );
            tally(&mut hits, &groups);

            if !filters.is_empty() && !filters.iter().any(|filter| file.starts_with(filter)) {
                continue;
            }

            if let Some(pattern) = &pattern {
                mark_matches(&mut groups, &source.lines, pattern);
                groups.retain(|group| !group.matches.is_empty());
                if groups.is_empty() {
                    continue;
                }
            }

            if statements {
                self.extend_to_statements(&file, &positions, &mut groups);
            }

            hits.shown.push(FileHits {
                groups,
                path,
                source: Rc::clone(&source.lines),
            });
        }

        Ok(hits)
    }

    /// Follow each reference in `groups` to the end of the statement it starts,
    /// from the selection ranges the server gives for `positions`.
    ///
    /// A server that does not answer leaves the groups as they are, with the
    /// fixed context around each reference.
    fn extend_to_statements(
        &mut self,
        file: &Utf8Path,
        positions: &[Position],
        groups: &mut [Group],
    ) {
        let Ok(uri) = uri(file) else {
            return;
        };
        let Ok(Some(selections)) = self.server.request::<Option<Vec<SelectionRange>>>(
            "textDocument/selectionRange",
            &json!({ "textDocument": { "uri": uri }, "positions": positions }),
        ) else {
            return;
        };
        if selections.len() != positions.len() {
            return;
        }

        let mut ends: BTreeMap<usize, usize> = BTreeMap::new();
        for (position, selection) in positions.iter().zip(&selections) {
            let end = to_usize(statement_end(&selection.chain(), position.line));
            let entry = ends.entry(to_usize(position.line)).or_insert(end);
            *entry = (*entry).max(end);
        }

        for group in groups {
            for (line, end) in group.lines.iter().zip(group.ends.iter_mut()) {
                if let Some(&statement) = ends.get(line) {
                    *end = (*end).max(statement);
                }
            }
        }
    }

    /// The fields, variants and methods of the type or trait at `position`,
    /// each with its references.
    ///
    /// A struct's or enum's methods are those of its inherent `impl` blocks;
    /// methods of a trait impl are the trait's.
    fn members(
        &mut self,
        file: &Utf8Path,
        position: Position,
        filters: &[Utf8PathBuf],
    ) -> Result<Members, String> {
        let readable = self
            .relative(file)
            .is_some_and(|path| self.readable(&path).is_ok());
        if !readable {
            return Ok(Members {
                note: Some("the symbol is outside the files this tool may read.".to_owned()),
                ..Members::default()
            });
        }

        let source = self.source(file)?;
        let Some(index) = outline::at(&source.items, position) else {
            return Ok(Members {
                note: Some("no item is defined at the symbol's position.".to_owned()),
                ..Members::default()
            });
        };
        let item = &source.items[index];
        if !matches!(item.kind, kind::STRUCT | kind::ENUM | kind::INTERFACE) {
            return Ok(Members {
                note: Some(format!(
                    "`{}` is a {}; only structs, enums and traits have members.",
                    item.name,
                    kind_label(item.kind)
                )),
                ..Members::default()
            });
        }

        // Each member as the file it is in and its index there.
        let mut found = children(&source.items, index)
            .map(|child| (file.to_owned(), Rc::clone(&source), child))
            .collect::<Vec<_>>();

        if item.kind != kind::INTERFACE {
            for location in self.implementation_locations(file, item.selection.start)? {
                let Ok(impl_file) = file_path(&location.uri) else {
                    continue;
                };
                let impl_readable = self
                    .relative(&impl_file)
                    .is_some_and(|path| self.readable(&path).is_ok());
                if !impl_readable {
                    continue;
                }

                let impl_source = self.source(&impl_file)?;
                let Some(block) = outline::at(&impl_source.items, location.range.start) else {
                    continue;
                };
                let inherent = impl_source.items[block].kind == kind::OBJECT
                    && matches!(
                        rust_analyzer::impl_parts(&impl_source.items[block].ident),
                        Some((None, _))
                    );
                if inherent {
                    found.extend(
                        children(&impl_source.items, block)
                            .map(|child| (impl_file.clone(), Rc::clone(&impl_source), child)),
                    );
                }
            }
        }

        let omitted = found.len().saturating_sub(MEMBERS);
        found.truncate(MEMBERS);

        let mut members = vec![];
        for (member_file, member_source, child) in found {
            let child = &member_source.items[child];
            let hits = self.references(&member_file, child.selection.start, filters, false)?;

            members.push(Member {
                label: format!("{} {}", kind_label(child.kind), child.ident),
                location: self.display(&member_file, child.selection.start.line),
                outside_tests: hits.outside_tests.count,
                in_tests: hits.in_tests.count,
                withheld: hits.withheld,
                places: places(&hits),
            });
        }

        Ok(Members {
            note: None,
            members,
            omitted,
        })
    }

    /// A file's text and items, read and asked for once per call.
    fn source(&mut self, file: &Utf8Path) -> Result<Rc<Source>, String> {
        if let Some(source) = self.sources.get(file) {
            return Ok(Rc::clone(source));
        }

        let text = fs::read_to_string(file)
            .map_err(|error| format!("Could not read `{file}`: {error}"))?;
        let symbols = self
            .server
            .request::<Option<Vec<DocumentSymbol>>>(
                "textDocument/documentSymbol",
                &json!({ "textDocument": { "uri": uri(file)? } }),
            )
            .map_err(|failure| failure.to_string())?
            .unwrap_or_default();

        let source = Rc::new(Source {
            lines: text.lines().map(str::to_owned).collect(),
            items: outline::flatten(&symbols),
        });
        self.sources.insert(file.to_owned(), Rc::clone(&source));

        Ok(source)
    }

    /// Resolve a workspace-relative path the tool will read, under the access
    /// policy.
    fn readable(&self, path: &str) -> Result<Utf8PathBuf, String> {
        let resolved = resolve_workspace_path(self.root, path, self.access)?;
        authorize(self.access, Capability::Read, &resolved)?;

        Ok(resolved.absolute)
    }

    /// `file` relative to the workspace root, or `None` outside it.
    fn relative(&self, file: &Utf8Path) -> Option<String> {
        file.strip_prefix(self.root)
            .or_else(|_| file.strip_prefix(&self.canonical_root))
            .ok()
            .map(|path| path.as_str().to_owned())
    }

    /// `path:line` with a one-based line, the way the other tools write it.
    fn display(&self, file: &Utf8Path, line: u32) -> String {
        match self.relative(file) {
            Some(path) => format!("{path}:{}", line + 1),
            None => format!("{file}:{}, outside the workspace", line + 1),
        }
    }
}

/// The references in one file, grouped by the item that encloses them.
///
/// Groups are ordered by their first line.
/// A reference outside any item gets a group of its own per file.
fn group_lines(items: &[Item], source: &[String], lines: &[u32], test_file: bool) -> Vec<Group> {
    let mut groups: BTreeMap<Option<usize>, Group> = BTreeMap::new();

    for &line in lines {
        let enclosing = outline::enclosing(items, line);
        let group = groups.entry(enclosing).or_insert_with(|| {
            let item = enclosing.map(|index| &items[index]);
            Group {
                item: item.map(|item| GroupItem {
                    label: item_label(item),
                    start: to_usize(outline::code_start(item, source)),
                    end: to_usize(item.range.end.line),
                }),
                lines: vec![],
                ends: vec![],
                matches: vec![],
                count: 0,
                test: test_file || item.is_some_and(|item| item.in_tests || is_test_module(item)),
            }
        });

        group.count += 1;
        let line = to_usize(line);
        if let Err(at) = group.lines.binary_search(&line) {
            group.lines.insert(at, line);
            group.ends.insert(at, line);
        }
    }

    let mut groups = groups.into_values().collect::<Vec<_>>();
    groups.sort_by_key(|group| group.lines.first().copied());
    groups
}

/// Record in each group the lines `pattern` matches: in its item's code, or for
/// a group outside any item, on its referencing lines.
fn mark_matches(groups: &mut [Group], source: &[String], pattern: &Regex) {
    for group in groups {
        let lines = match &group.item {
            Some(item) => (item.start..=item.end).collect::<Vec<_>>(),
            None => group.lines.clone(),
        };

        group.matches = lines
            .into_iter()
            .filter(|&line| {
                source
                    .get(line)
                    .is_some_and(|text| pattern.is_match(text).unwrap_or(false))
            })
            .collect();
    }
}

/// `fn total`, `struct Stream`, or an `impl` block's own name: `impl Render for
/// Stream`.
fn item_label(item: &Item) -> String {
    if item.kind == kind::OBJECT {
        item.name.clone()
    } else {
        format!("{} {}", kind_label(item.kind), item.name)
    }
}

/// The direct children of item `parent` that are members: everything but
/// generic parameters.
fn children(items: &[Item], parent: usize) -> impl Iterator<Item = usize> + '_ {
    items
        .iter()
        .enumerate()
        .filter(move |(_, item)| item.parent == Some(parent) && item.kind != kind::TYPE_PARAMETER)
        .map(|(index, _)| index)
}

/// The items outside tests holding a hit, with where the first hit is: `fn
/// total (src/stream.rs:28)`.
fn places(hits: &Hits) -> Vec<String> {
    hits.shown
        .iter()
        .flat_map(|file| {
            file.groups
                .iter()
                .filter(|group| !group.test)
                .map(move |group| {
                    let line = group.lines.first().map_or(0, |line| line + 1);
                    match &group.item {
                        Some(item) => format!("{} ({}:{line})", item.label, file.path),
                        None => format!("{}:{line}", file.path),
                    }
                })
        })
        .collect()
}

/// Count one file's groups toward the hits outside tests and in them.
fn tally(hits: &mut Hits, groups: &[Group]) {
    for (test, tally) in [(false, &mut hits.outside_tests), (true, &mut hits.in_tests)] {
        let count = groups
            .iter()
            .filter(|group| group.test == test)
            .map(|group| group.count)
            .sum::<usize>();
        if count > 0 {
            tally.count += count;
            tally.files += 1;
        }
    }
}

fn is_function_kind(value: u32) -> bool {
    matches!(value, kind::FUNCTION | kind::METHOD | kind::CONSTRUCTOR)
}

/// A reference that sits directly in a `tests` module, outside any function.
fn is_test_module(item: &Item) -> bool {
    item.kind == kind::MODULE && rust_analyzer::is_test_module(&item.ident)
}

/// The last line of the statement a reference on `line` starts, from the
/// selection ranges the server nests around it, innermost first.
///
/// That is the largest range starting on `line` that ends at most
/// [`STATEMENT_LINES`] lines further down; `line` itself when none does.
fn statement_end(chain: &[Range], line: u32) -> u32 {
    chain
        .iter()
        .filter(|range| range.start.line == line)
        .map(|range| range.end.line)
        .filter(|&end| end >= line && end - line <= STATEMENT_LINES)
        .max()
        .unwrap_or(line)
}

/// `text` with `edits` applied, as [`edit_spans`] reads them.
#[cfg(test)]
fn apply_edits(
    text: &str,
    edits: &[TextEdit],
    encoding: PositionEncoding,
    old: &str,
) -> Result<String, String> {
    Ok(splice(text, &edit_spans(text, edits, encoding, old)?))
}

/// The byte range each of `edits` replaces in `text`, with its replacement,
/// sorted, their positions counted in `encoding`.
///
/// An edit that replaces text must replace text containing `old`, the name
/// being renamed.
/// One that does not means the file changed after the server read it, and
/// nothing is applied.
fn edit_spans<'e>(
    text: &str,
    edits: &'e [TextEdit],
    encoding: PositionEncoding,
    old: &str,
) -> Result<Vec<(usize, usize, &'e str)>, String> {
    let starts = line_starts(text);
    let mut spans = edits
        .iter()
        .map(|edit| {
            let start = byte_at(text, &starts, edit.range.start, encoding)?;
            let end = byte_at(text, &starts, edit.range.end, encoding)?;
            if end < start {
                return Err(format!(
                    "an edit at line {} ends before it starts",
                    edit.range.start.line + 1
                ));
            }

            let replaced = &text[start..end];
            if !replaced.is_empty() && !replaced.contains(old) {
                return Err(format!(
                    "line {} reads `{replaced}` where the rename expected `{old}`; the file \
                     changed after rust-analyzer read it. Nothing was renamed.",
                    edit.range.start.line + 1
                ));
            }

            Ok((start, end, edit.new_text.as_str()))
        })
        .collect::<Result<Vec<_>, String>>()?;
    spans.sort_by_key(|&(start, end, _)| (start, end));

    if spans.windows(2).any(|pair| pair[1].0 < pair[0].1) {
        return Err("two edits overlap. Nothing was renamed.".to_owned());
    }

    Ok(spans)
}

/// `text` with each sorted, non-overlapping span replaced.
fn splice(text: &str, spans: &[(usize, usize, &str)]) -> String {
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    for &(start, end, new_text) in spans {
        out.push_str(&text[at..start]);
        out.push_str(new_text);
        at = end;
    }
    out.push_str(&text[at..]);

    out
}

/// Where `position` in `before` is in `after`, the text `spans` made of it.
///
/// `None` when a span replaces text at or around the position: the name there
/// was itself rewritten.
fn shift_position(
    before: &str,
    after: &str,
    spans: &[(usize, usize, &str)],
    position: Position,
    encoding: PositionEncoding,
) -> Option<Position> {
    let offset = byte_at(before, &line_starts(before), position, encoding).ok()?;

    let (mut added, mut removed) = (0, 0);
    for &(start, end, new_text) in spans {
        if end <= offset {
            added += new_text.len();
            removed += end - start;
        } else if start <= offset {
            return None;
        } else {
            break;
        }
    }

    let moved = offset + added - removed;
    let line_start = after[..moved].rfind('\n').map_or(0, |at| at + 1);
    let line_end = after[line_start..]
        .find('\n')
        .map_or(after.len(), |at| line_start + at);

    Some(Position {
        line: to_u32(after[..moved].matches('\n').count()),
        character: encoding.character(&after[line_start..line_end], moved - line_start),
    })
}

/// The byte offset each line of `text` starts at.
fn line_starts(text: &str) -> Vec<usize> {
    iter::once(0)
        .chain(text.match_indices('\n').map(|(at, _)| at + 1))
        .collect()
}

/// The byte offset in `text` of `position`.
fn byte_at(
    text: &str,
    starts: &[usize],
    position: Position,
    encoding: PositionEncoding,
) -> Result<usize, String> {
    let start = *starts
        .get(to_usize(position.line))
        .ok_or_else(|| format!("line {} is past the end of the file", position.line + 1))?;
    let end = text[start..].find('\n').map_or(text.len(), |at| start + at);

    encoding
        .byte(&text[start..end], position.character)
        .map(|offset| start + offset)
        .ok_or_else(|| {
            format!(
                "line {} has no character {}",
                position.line + 1,
                position.character
            )
        })
}

/// Where each location starts, grouped by file in path order and in order
/// within a file.
fn by_file(locations: Vec<Location>) -> BTreeMap<Utf8PathBuf, Vec<Position>> {
    let mut files: BTreeMap<Utf8PathBuf, Vec<Position>> = BTreeMap::new();
    for location in locations {
        if let Ok(path) = file_path(&location.uri) {
            files.entry(path).or_default().push(location.range.start);
        }
    }

    for positions in files.values_mut() {
        positions.sort_unstable();
    }

    files
}

fn position_params(file: &Utf8Path, position: Position) -> Result<Value, String> {
    Ok(json!({ "textDocument": { "uri": uri(file)? }, "position": position }))
}

fn uri(file: &Utf8Path) -> Result<String, String> {
    Url::from_file_path(file)
        .map(String::from)
        .map_err(|()| format!("`{file}` is not an absolute path."))
}

fn file_path(uri: &Url) -> Result<Utf8PathBuf, String> {
    uri.to_file_path()
        .ok()
        .and_then(|path| Utf8PathBuf::from_path_buf(path).ok())
        .ok_or_else(|| format!("`{uri}` is not a local file."))
}

fn to_u32(value: usize) -> u32 {
    u32::try_from(value).unwrap_or(u32::MAX)
}

fn to_usize(value: u32) -> usize {
    usize::try_from(value).unwrap_or(usize::MAX)
}

#[cfg(test)]
#[path = "code_tests.rs"]
mod tests;
