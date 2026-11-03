//! `jp wiki edit` and `jp wiki rename`: change a page by its flags, or in the
//! user's editor.
//!
//! Either way the page is checked before it is kept, the way `jp wiki check`
//! checks it, and only for problems the edit brings: one already there before
//! doesn't stop an unrelated change.
//! A page becoming a Snapshot is pinned before it is checked, so the check sees
//! the permalinks it will hold.
//! What an edit implies besides the file follows it ([`follow_up`]): a new
//! title moves the file and the links to it, and an archived page or one with a
//! new parent leaves its place in the order file.
//!
//! What the edit writes of the page itself, its file and its place in the order
//! file, is asked of the command's access before anything is written.
//! Links in other files are rewritten where the access allows, and the files it
//! doesn't are named.

use std::{
    collections::HashSet,
    fs,
    io::{self, BufRead, Write},
};

use camino::{Utf8Path, Utf8PathBuf};
use doc::{
    metadata,
    references::{self, Rule, names_stem},
};
use jp_plugin::message::{
    ComposeMode, ComposeOption, ComposeRequest, EditFilesRequest, HostToPlugin, PluginToHost,
};
use jp_tool::Capability;

use crate::{
    Context, Output,
    check::{self, Finding, check_page},
    cmd::{self, active_parent, refuse_active_children, superseded_target, unique},
    library::Library,
    order::ORDER_FILE,
    page::{Page, PageId, Status, body_start},
    pin::{self, Pinned},
    read_message, section, send, store,
    store::Wiki,
};

/// The protocol that added `edit_files`, through which the host opens the
/// editor.
pub(crate) const EDIT_FILES_PROTOCOL: u32 = 13;

/// What to tell someone who can't get an editor: the flags do the same.
const USE_THE_FLAGS: &str =
    "Pass what to change instead: --title, --summary, --status, --body, and the rest.";

/// What `edit` changes; `None`, or no labels, leaves that part as it is.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Changes {
    pub title: Option<String>,
    pub summary: Option<String>,
    pub status: Option<Status>,

    /// `Some(None)` takes the parent away.
    pub parent: Option<Option<PageId>>,

    /// `Some(None)` takes the field away.
    pub superseded_by: Option<Option<String>>,

    /// The new body, or with `section`, the new text of that section.
    pub body: Option<String>,
    pub section: Option<String>,

    pub add_labels: Vec<String>,

    /// A `key=value` removes that label, a bare `key` every label under it.
    pub remove_labels: Vec<String>,
}

/// Change page `id` by `changes`.
pub(crate) fn edit(ctx: &Context<'_>, id: PageId, changes: &Changes) -> Result<Output, String> {
    if changes.section.is_some() && changes.body.is_none() {
        return Err(
            "`--section` names the section `--body` replaces; pass the body too.".to_owned(),
        );
    }

    let wiki = ctx.wiki()?;
    let before = wiki.find(id)?.clone();
    let library = ctx.library();
    let text = apply(ctx, &wiki, &library, &before, changes)?;
    let (after, pinned) = pin_snapshot(ctx, &before, Page::read(id, before.path.clone(), text))?;

    let review = review(ctx, &wiki, &library, &before, &after)?;
    if !review.problems.is_empty() {
        return Err(format!(
            "{id} is unchanged: {}",
            review.explained(pinned.as_ref())
        ));
    }

    kept(ctx, wiki, &before, &after, pinned, review)
}

/// `after`, pinned when it is a page becoming a Snapshot, with what the pin
/// did.
fn pin_snapshot(
    ctx: &Context<'_>,
    before: &Page,
    after: Page,
) -> Result<(Page, Option<Pinned>), String> {
    let becomes_snapshot =
        after.status() == Some(Status::Snapshot) && before.status() != Some(Status::Snapshot);
    if !becomes_snapshot {
        return Ok((after, None));
    }

    let repository = ctx.repository()?;
    let (text, report) = pin::pin(
        &after.source,
        &after.path,
        &ctx.paths,
        repository.as_deref(),
        &ctx.access,
    );

    Ok((Page::read(after.id, after.path, text), Some(report)))
}

/// Keep `after` and report it: what [`follow_up`] did, the links pinned, and
/// what couldn't be checked.
fn kept(
    ctx: &Context<'_>,
    wiki: Wiki,
    before: &Page,
    after: &Page,
    pinned: Option<Pinned>,
    review: Review,
) -> Result<Output, String> {
    let mut output = follow_up(ctx, wiki, before, after)?;
    if let Some(report) = pinned {
        output.extend(cmd::pin_report(report));
    }
    output.warnings.extend(review.unchecked);

    Ok(output)
}

/// The text of `page` with `changes` applied, each checked as it is made.
fn apply(
    ctx: &Context<'_>,
    wiki: &Wiki,
    library: &Library,
    page: &Page,
    changes: &Changes,
) -> Result<String, String> {
    let mut document = metadata::Document::parse(&page.source);
    let no_block = |_| format!("{} has no metadata block.", ctx.paths.show(&page.path));

    if let Some(title) = &changes.title {
        retitle(&mut document, cmd::check_title(title)?)?;
    }
    if let Some(summary) = &changes.summary {
        let summary = summary.trim();
        if summary.is_empty() || summary.contains(['\n', '\r']) {
            return Err("A summary is one sentence on one line.".to_owned());
        }
        document.set("Summary", summary).map_err(no_block)?;
    }
    if let Some(status) = changes.status {
        document
            .set("Status", &status.to_string())
            .map_err(no_block)?;
    }
    match changes.parent {
        Some(Some(parent)) => {
            reparent(wiki, page.id, parent)?;
            document
                .set("Parent", &parent.to_string())
                .map_err(no_block)?;
        }
        Some(None) => {
            document.remove("Parent");
        }
        None => {}
    }
    match &changes.superseded_by {
        Some(Some(given)) => {
            let target = superseded_target(wiki, library, page.id, given)?;
            document.set("Superseded by", &target).map_err(no_block)?;
        }
        Some(None) => {
            document.remove("Superseded by");
        }
        None => {}
    }

    if !changes.add_labels.is_empty() || !changes.remove_labels.is_empty() {
        let tokens = relabel(&page.labels, &changes.add_labels, &changes.remove_labels);
        let labels = ctx
            .options
            .vocabulary()?
            .resolve_against(&tokens, &page.label_set())
            .map_err(|error| error.to_string())?;
        document
            .set_repeated(crate::page::LABEL_KEY, &labels.to_tokens())
            .map_err(no_block)?;
    }

    let text = document.render();
    let Some(body) = &changes.body else {
        return Ok(text);
    };

    match &changes.section {
        Some(wanted) => {
            let found = section::find(&text, wanted)?;
            Ok(section::replace(&text, &found, body))
        }
        None => Ok(replace_body(&text, body)),
    }
}

/// Give the document the title `title`, on its `# Title` line.
fn retitle(document: &mut metadata::Document, title: &str) -> Result<(), String> {
    let lines = document.lines_mut();
    let Some(line) = lines.iter_mut().find(|line| !line.trim().is_empty()) else {
        return Err("The page is empty.".to_owned());
    };
    if !line.starts_with("# ") {
        return Err("The page doesn't open with a `# Title` heading to change.".to_owned());
    }

    *line = format!("# {title}");
    Ok(())
}

/// Refuse `parent` as the parent of `id` when it isn't an active page, or is
/// `id` itself or one of its own children's children.
fn reparent(wiki: &Wiki, id: PageId, parent: PageId) -> Result<(), String> {
    if parent == id {
        return Err(format!("{id} can't be its own parent."));
    }
    active_parent(wiki, parent)?;

    let mut seen = HashSet::new();
    let mut at = parent;
    while let Ok(page) = wiki.find(at)
        && let Some(up) = page.parent_id()
    {
        if up == id {
            return Err(format!(
                "{parent} sits below {id}, so it can't be its parent."
            ));
        }
        if !seen.insert(up) {
            break;
        }
        at = up;
    }

    Ok(())
}

/// `current` with the labels `remove` names taken away and `add` added.
fn relabel(current: &[String], add: &[String], remove: &[String]) -> Vec<String> {
    let key = |token: &str| {
        token
            .split_once('=')
            .map_or(token, |(key, _)| key)
            .trim()
            .to_owned()
    };
    let removed = |token: &String| {
        remove.iter().any(|removal| {
            if removal.contains('=') {
                token.trim().eq_ignore_ascii_case(removal.trim())
            } else {
                key(token).eq_ignore_ascii_case(removal.trim())
            }
        })
    };

    let mut tokens: Vec<String> = current
        .iter()
        .filter(|token| !removed(token))
        .cloned()
        .collect();
    for token in add {
        if !tokens
            .iter()
            .any(|kept| kept.trim().eq_ignore_ascii_case(token.trim()))
        {
            tokens.push(token.clone());
        }
    }

    tokens
}

/// `text` with everything after its title and metadata block replaced by
/// `body`.
fn replace_body(text: &str, body: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let mut out = lines[..body_start(&lines)].join("\n").trim_end().to_owned();

    let body = body.trim();
    if !body.is_empty() {
        out.push_str("\n\n");
        out.push_str(body);
    }
    out.push('\n');

    out
}

/// What checking an edit found.
struct Review {
    /// What keeps the edit from being kept.
    problems: Vec<String>,

    /// What the edit brings that couldn't be checked, because the command may
    /// not read what it points at.
    unchecked: Vec<String>,
}

impl Review {
    /// The problems, as one message, with why each link the edit's pin left
    /// alone couldn't be pinned: a Snapshot citing source by a relative link is
    /// one of the problems, and the reason is what to act on.
    fn explained(&self, pinned: Option<&Pinned>) -> String {
        let mut message = self.problems.join(" ");
        for (url, why) in pinned
            .map(|report| report.left.as_slice())
            .unwrap_or_default()
        {
            message.push_str(&format!(" `{url}` couldn't be pinned: {why}."));
        }

        message
    }
}

/// Check `after` against `before`.
///
/// What keeps it from replacing `before`: problems `check` finds in it that it
/// didn't find before, an archived page with active children, and labels
/// another page holds that the vocabulary lets one page carry.
fn review(
    ctx: &Context<'_>,
    wiki: &Wiki,
    library: &Library,
    before: &Page,
    after: &Page,
) -> Result<Review, String> {
    let repository = ctx.repository()?;
    let context = check::Context {
        paths: &ctx.paths,
        options: ctx.options,
        wiki,
        library,
        repository: repository.as_deref(),
        access: &ctx.access,
    };

    let earlier = check_page(&context, before);
    let later = check_page(&context, after);
    let mut problems = fresh(&earlier.errors, later.errors);
    let unchecked = fresh(&earlier.unchecked, later.unchecked);

    if after.is_archived()
        && !before.is_archived()
        && let Err(error) = refuse_active_children(wiki, after.id)
    {
        problems.push(error);
    }
    if after.labels != before.labels
        && let Ok(vocabulary) = ctx.options.vocabulary()
        && let Err(error) = unique(&vocabulary, &after.label_set(), wiki, Some(after.id))
    {
        problems.push(error);
    }

    Ok(Review {
        problems,
        unchecked,
    })
}

/// The findings in `later` that `earlier` doesn't hold, as messages naming
/// their line.
fn fresh(earlier: &[Finding], later: Vec<Finding>) -> Vec<String> {
    let known: HashSet<&str> = earlier
        .iter()
        .map(|finding| finding.message.as_str())
        .collect();

    later
        .into_iter()
        .filter(|finding| !known.contains(finding.message.as_str()))
        .map(|finding| match finding.line {
            Some(line) => format!("Line {line}: {}", finding.message),
            None => finding.message,
        })
        .collect()
}

/// Keep `after`, and do what it implies besides the file: a new title moves the
/// file and rewrites links to it, and the order file loses an archived page and
/// unranks a moved one.
///
/// What the edit writes of the page itself is asked of the command's access
/// before anything is written, so a refusal leaves the wiki as it was.
fn follow_up(
    ctx: &Context<'_>,
    mut wiki: Wiki,
    before: &Page,
    after: &Page,
) -> Result<Output, String> {
    let id = after.id;
    let from = before.path.clone();
    let to = match &after.title {
        Some(title) if after.title != before.title => store::path_for(&ctx.paths.wiki, id, title),
        _ => from.clone(),
    };

    let archived = after.is_archived() && !before.is_archived();
    let moved = after.parent_id() != before.parent_id();
    let reordered = if archived {
        wiki.order.remove(id)
    } else if moved {
        wiki.order.unrank(id)
    } else {
        false
    };

    let order_file = ctx.paths.wiki.join(ORDER_FILE);
    let mut asked = vec![(Capability::Update, from.as_path())];
    if to != from {
        asked.push((Capability::Delete, from.as_path()));
        asked.push((Capability::Create, to.as_path()));
    }
    if reordered {
        asked.push((Capability::Update, order_file.as_path()));
    }
    for (capability, path) in asked {
        ctx.access
            .check(capability, path)
            .map_err(|refusal| format!("{id} is unchanged: {refusal}."))?;
    }

    let mut output = Output::default();
    ctx.paths.write(&from, &after.source)?;
    if to != from {
        fs::rename(&from, &to).map_err(|error| format!("{from}: {error}"))?;
        output.extend(rewrite_links(ctx, &from, &to)?);
    }
    if reordered {
        wiki.order.write(&ctx.paths.wiki)?;
    }

    output.text = format!("Edited {}\n{}", ctx.paths.show(&to), output.text);

    Ok(output)
}

/// Point the links to the page that moved from `from` to `to` at its new file,
/// across the documentation tree and the RFD and ticket directories, and name
/// the files that still link to the old one: those there the command may not
/// update, and those elsewhere in the workspace.
///
/// A file the command may not read can't be searched, and how many there were
/// is said; so is a workspace it may not read at all.
fn rewrite_links(ctx: &Context<'_>, from: &Utf8Path, to: &Utf8Path) -> Result<Output, String> {
    let paths = &ctx.paths;
    let access = &ctx.access;
    let old = from.file_stem().unwrap_or_default().to_owned();
    let new = to.file_stem().unwrap_or_default().to_owned();

    let docs = paths.docs();
    let mut linking: Vec<Utf8PathBuf> = vec![docs.clone()];
    for dir in [&paths.rfds, &paths.tickets] {
        if !dir.starts_with(&docs) && dir.is_dir() {
            linking.push(dir.clone());
        }
    }

    let rules = [Rule::Stem {
        stem: old.clone(),
        to: new,
    }];
    let mut output = Output::default();
    let mut left = vec![];
    let mut unread = 0;
    for file in linking.iter().flat_map(|dir| references::walk(dir)) {
        if !access.reads(&file) {
            unread += 1;
            continue;
        }
        let source = match fs::read_to_string(&file) {
            Ok(source) => source,
            // A file that isn't text holds no links.
            Err(error) if error.kind() == io::ErrorKind::InvalidData => continue,
            Err(error) => return Err(format!("{file}: {error}")),
        };
        let rewritten = references::apply(&source, &rules);
        if rewritten == source {
            continue;
        }

        match access.check(Capability::Update, &file) {
            Ok(()) => {
                paths.write(&file, &rewritten)?;
                output
                    .text
                    .push_str(&format!("  updated links in {}\n", paths.show(&file)));
            }
            Err(refusal) => left.push(format!("  {} ({refusal})", paths.show(&file))),
        }
    }

    let outside_unchecked = if access.reads_workspace() {
        for file in references::walk(&paths.root) {
            if linking.iter().any(|dir| file.starts_with(dir)) {
                continue;
            }
            if !access.reads(&file) {
                unread += 1;
                continue;
            }
            if fs::read_to_string(&file).is_ok_and(|text| names_stem(&text, &old)) {
                left.push(format!("  {}", paths.show(&file)));
            }
        }
        None
    } else {
        let scope: Vec<String> = linking
            .iter()
            .map(|dir| format!("{}/", paths.show(dir)))
            .collect();
        Some(format!(
            "References outside {} were not checked: the access policy doesn't allow reading the \
             workspace.",
            scope.join(" and ")
        ))
    };

    if !left.is_empty() {
        output.warnings.push(format!(
            "These files still link to {old}, and weren't rewritten:\n{}",
            left.join("\n")
        ));
    }
    output.warnings.extend(outside_unchecked);
    match unread {
        0 => {}
        1 => output.warnings.push(format!(
            "1 file couldn't be read to look for links to {old}."
        )),
        count => output.warnings.push(format!(
            "{count} files couldn't be read to look for links to {old}."
        )),
    }

    Ok(output)
}

/// How an edit reaches the user while the page is open.
pub(crate) trait Editing {
    /// Open `path` in the user's editor and wait for it to close.
    ///
    /// `true` when the editor exited cleanly.
    fn open(&mut self, path: &Utf8Path) -> Result<bool, String>;

    /// Ask whether to open the editor again, after `problem` was found in what
    /// it left behind.
    fn again(&mut self, problem: &str) -> bool;
}

/// [`Editing`] through the host: `edit_files` opens the editor, and `compose`
/// asks the question.
pub(crate) struct Host<'a, R, W> {
    pub(crate) stdin: &'a mut R,
    pub(crate) stdout: &'a mut W,
}

impl<R: BufRead, W: Write> Editing for Host<'_, R, W> {
    fn open(&mut self, path: &Utf8Path) -> Result<bool, String> {
        send(
            self.stdout,
            &PluginToHost::EditFiles(EditFilesRequest {
                id: None,
                paths: vec![path.to_owned()],
            }),
        )?;

        match read_message(self.stdin)? {
            HostToPlugin::Edited(response) => Ok(response.saved),
            HostToPlugin::Error(error) => Err(format!(
                "Couldn't open an editor: {}. {USE_THE_FLAGS}",
                error.message
            )),
            HostToPlugin::Shutdown => Err("Interrupted.".to_owned()),
            other => Err(format!("expected an edited response, got: {other:?}")),
        }
    }

    fn again(&mut self, problem: &str) -> bool {
        let option = |value: &str, label: &str| ComposeOption {
            value: value.to_owned(),
            label: label.to_owned(),
        };
        let asked = send(
            self.stdout,
            &PluginToHost::Compose(ComposeRequest {
                id: None,
                message: problem.to_owned(),
                mode: ComposeMode::Select {
                    options: vec![
                        option("edit", "Open the editor again"),
                        option("discard", "Discard the edit"),
                    ],
                    default: Some("edit".to_owned()),
                },
                help: None,
            }),
        );

        // No answer, from a cancelled prompt or a missing terminal, discards.
        asked.is_ok()
            && matches!(
                read_message(self.stdin),
                Ok(HostToPlugin::Composed(response)) if response.text.as_deref() == Some("edit")
            )
    }
}

/// Open page `id` in the editor of the host that sent `init`, speaking protocol
/// `version`.
pub(crate) fn through_host(
    version: u32,
    ctx: &Context<'_>,
    id: PageId,
    stdin: &mut impl BufRead,
    stdout: &mut impl Write,
) -> Result<Output, String> {
    if version < EDIT_FILES_PROTOCOL {
        return Err(format!(
            "Opening an editor needs `jp` protocol {EDIT_FILES_PROTOCOL}, and this `jp` speaks \
             {version}; reinstall the two together. {USE_THE_FLAGS}"
        ));
    }

    in_editor(ctx, id, &mut Host { stdin, stdout })
}

/// Open page `id` in the user's editor until what comes back is a page `check`
/// accepts, or the user gives up on the edit.
///
/// The editor writes the file itself; an edit given up on, or one the command
/// fails on before it is accepted, is undone.
pub(crate) fn in_editor(
    ctx: &Context<'_>,
    id: PageId,
    editing: &mut impl Editing,
) -> Result<Output, String> {
    let wiki = ctx.wiki()?;
    let before = wiki.find(id)?.clone();
    let library = ctx.library();
    let path = before.path.clone();
    let restore = |error: String| match fs::write(&path, &before.source) {
        Ok(()) => error,
        Err(restore) => format!("{error}\nfailed to restore {path}: {restore}"),
    };

    loop {
        if !editing.open(&path).map_err(&restore)? {
            return Err(restore(format!(
                "The editor exited with an error; {id} is unchanged."
            )));
        }

        let edited = store::read(&path).map_err(&restore)?;
        if edited == before.source {
            return Ok(format!("{}: unchanged\n", ctx.paths.show(&path)).into());
        }

        let (after, pinned) =
            pin_snapshot(ctx, &before, Page::read(id, path.clone(), edited)).map_err(&restore)?;
        let review = review(ctx, &wiki, &library, &before, &after).map_err(&restore)?;
        if review.problems.is_empty() {
            return kept(ctx, wiki, &before, &after, pinned, review);
        }

        let problems = review.explained(pinned.as_ref());
        if !editing.again(&format!("{id} can't be saved: {problems}")) {
            return Err(restore(format!("{id} is unchanged: {problems}")));
        }
    }
}

#[cfg(test)]
#[path = "edit_tests.rs"]
mod tests;
