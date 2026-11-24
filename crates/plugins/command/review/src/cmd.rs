//! Review-level CLI commands: `start`, `ls`, `show`, `diff`, `close`, and
//! `open`.
//!
//! Comment and reply commands live in [`crate::comments`].

use std::{collections::BTreeMap, env};

use camino::{Utf8Path, Utf8PathBuf};
use doc::author;
use jp_plugin::message::{InitMessage, OutputFormat};
use jp_review_api::DiffView;
use serde::Serialize;
use serde_json::Value;

use crate::{
    Output,
    browser::Browser,
    id::ReviewId,
    model::{BASE_LABEL, Review, Status},
    options::Options,
    pass::PassRef,
    paths,
    render::{self, counted},
    snapshot,
    store::{self, Blobs, Store, now},
    view::{self, Texts},
};

/// Execution context for `jp review` CLI commands.
pub(crate) struct Context<'a> {
    /// Workspace root directory; tracked file paths are stored relative to this
    /// path.
    pub root: Utf8PathBuf,

    /// Working directory where `jp` was invoked; user-supplied paths are
    /// resolved relative to `cwd` before `root`.
    pub cwd: Utf8PathBuf,

    pub config: &'a Value,
    pub options: &'a Options,
    pub store: Store,
    pub format: OutputFormat,
}

impl<'a> Context<'a> {
    pub(crate) fn new(init: &'a InitMessage, options: &'a Options) -> Result<Self, String> {
        let root = init.workspace.root.clone();
        let cwd = env::current_dir()
            .ok()
            .and_then(|cwd| Utf8PathBuf::from_path_buf(cwd).ok())
            .unwrap_or_else(|| root.clone());
        let store = Store::new(options.store_dir(&root, init.paths.user_workspace.as_deref())?);

        Ok(Self {
            root,
            cwd,
            config: &init.config,
            options,
            store,
            format: init.output_format,
        })
    }

    /// Return `root` and `cwd` with symlinks resolved so relative paths between
    /// them can be computed reliably.
    pub(crate) fn bases(&self) -> (Utf8PathBuf, Utf8PathBuf) {
        (canonical(&self.root), canonical(&self.cwd))
    }
}

/// Canonicalize `path`, falling back to `path` as-is if canonicalization fails.
fn canonical(path: &Utf8Path) -> Utf8PathBuf {
    path.canonicalize_utf8()
        .unwrap_or_else(|_| path.to_path_buf())
}

pub(crate) fn json(value: &impl Serialize) -> Result<Output, String> {
    Ok(Output {
        text: to_json(value)?,
        json: true,
        ..Output::default()
    })
}

pub(crate) fn to_json(value: &impl Serialize) -> Result<String, String> {
    serde_json::to_string(value).map_err(|error| format!("failed to serialize the output: {error}"))
}

/// Start a new review of `files`, snapshotting the working tree (and the
/// optional `base` command output as version 1), returning its id and what the
/// command prints.
pub(crate) fn start(
    ctx: &Context<'_>,
    files: &[String],
    title: Option<&str>,
    base: Option<&[String]>,
) -> Result<(ReviewId, Output), String> {
    let paths = resolve_files(ctx, files)?;
    let (id, review) = create(ctx, paths, title, base)?;

    if ctx.format.is_json() {
        return Ok((id, json(&view::summary(id, &review))?));
    }

    let files = counted(review.paths.len(), "file", "files");
    let latest = review.latest().map_or(0, |latest| latest.number);
    let base = review
        .base()
        .map_or_else(String::new, |base| format!(" (version {base} is the base)"));
    Ok((
        id,
        format!("{id}: {files}, version {latest}{base}\n").into(),
    ))
}

/// Open the review list, or the review `id`, in `browser`, as the running `jp
/// review serve` serves it, returning the URL opened.
///
/// Refused when no server runs for the store, saying how to start one.
pub(crate) fn open(
    ctx: &Context<'_>,
    id: Option<ReviewId>,
    browser: &dyn Browser,
) -> Result<String, String> {
    if let Some(id) = id
        && !ctx.store.exists(id)
    {
        return Err(format!("No review {id}."));
    }
    let base = match ctx.store.server()? {
        None => {
            return Err(
                "No `jp review serve` runs for this workspace's reviews. Start one with `jp \
                 review serve --open`."
                    .to_owned(),
            );
        }
        Some(base) if base.is_empty() => {
            return Err(
                "`jp review serve` is starting, and hasn't said where it listens yet; try again."
                    .to_owned(),
            );
        }
        Some(base) => base,
    };
    let url = match id {
        Some(id) => format!("{base}/reviews/{id}"),
        None => format!("{base}/"),
    };

    browser.open(&url)?;
    Ok(url)
}

/// Create and persist a new review of workspace-relative `paths`, recording the
/// working tree (and the optional `base` command output as version 1).
pub(crate) fn create(
    ctx: &Context<'_>,
    paths: Vec<String>,
    title: Option<&str>,
    base: Option<&[String]>,
) -> Result<(ReviewId, Review), String> {
    let title = title
        .map(str::trim)
        .filter(|title| !title.is_empty())
        .map_or_else(|| default_title(&paths), ToOwned::to_owned);
    let author = author::resolve(&ctx.root, ctx.config)?;
    let at = now();

    let tree = snapshot::working_tree(&ctx.root, &paths)?;
    let base = base
        .map(|command| snapshot::base(&ctx.root, command, &paths))
        .transpose()?;

    // With `base` set, version 1 is always the base: a file the command
    // printed nothing for is absent from it, and reads as added.
    let mut review = Review::new(title, author, at, paths);
    if let Some(base) = &base {
        review.record(store::manifest(base), Some(BASE_LABEL), at);
    }
    review.record(store::manifest(&tree), None, at);
    let contents: Vec<&str> = base
        .iter()
        .flat_map(BTreeMap::values)
        .chain(tree.values())
        .flatten()
        .map(String::as_str)
        .collect();
    let id = ctx.store.create(&review, &contents)?;

    Ok((id, review))
}

/// Resolve user-supplied `files` to deduplicated workspace-relative paths in
/// the order given.
pub(crate) fn resolve_files(ctx: &Context<'_>, files: &[String]) -> Result<Vec<String>, String> {
    let (root, cwd) = ctx.bases();
    let mut resolved: Vec<String> = vec![];
    for file in files {
        let path = paths::existing(&root, &cwd, file)?;
        if !resolved.contains(&path) {
            resolved.push(path);
        }
    }

    Ok(resolved)
}

/// Default review title when `--title` is omitted (`<first>` or `<first> and N
/// more file(s)`).
fn default_title(paths: &[String]) -> String {
    match paths {
        [] => String::new(),
        [only] => only.clone(),
        [first, rest @ ..] => format!(
            "{first} and {}",
            counted(rest.len(), "more file", "more files")
        ),
    }
}

/// List every review in the store, oldest first.
///
/// A review that can't be read is reported, and the rest are listed.
pub(crate) fn list(ctx: &Context<'_>) -> Result<Output, String> {
    let mut warnings = vec![];
    let mut reviews = vec![];
    for entry in ctx.store.list()? {
        match entry.review {
            Ok(review) => reviews.push((entry.id, review)),
            Err(error) => warnings.push(error),
        }
    }

    let summaries: Vec<_> = reviews
        .iter()
        .map(|(id, review)| view::summary(*id, review))
        .collect();
    let mut output = if ctx.format.is_json() {
        json(&summaries)?
    } else {
        Output::from(summaries.iter().map(render::row).collect::<String>())
    };
    if summaries.is_empty() && !ctx.format.is_json() {
        output.notes.push("No reviews.".to_owned());
    }
    output.warnings = warnings;

    Ok(output)
}

/// Show a review, with its comments placed in version `asked`, or the latest.
pub(crate) fn show(ctx: &Context<'_>, id: ReviewId, asked: Option<u32>) -> Result<Output, String> {
    let review = ctx.store.read(id)?;
    let shown = version(id, &review, asked)?;
    let texts = load(&ctx.store.blobs(id), view::blobs_needed(&review, shown))?;
    let detail = view::detail(id, &review, shown, &texts);

    if ctx.format.is_json() {
        return json(&detail);
    }

    Ok(render::detail(&detail).into())
}

/// The version `asked` names, or the latest, refused when the review has no
/// such version.
pub(crate) fn version(id: ReviewId, review: &Review, asked: Option<u32>) -> Result<u32, String> {
    let latest = review
        .latest()
        .map(|latest| latest.number)
        .ok_or_else(|| format!("{id} has no versions."))?;
    let number = asked.unwrap_or(latest);
    if review.version(number).is_none() {
        return Err(format!(
            "{id} has no version {number}; its latest is {latest}."
        ));
    }

    Ok(number)
}

/// Read each of the blobs `names`.
pub(crate) fn load<'a>(
    blobs: &Blobs,
    names: impl IntoIterator<Item = &'a str>,
) -> Result<Texts, String> {
    names
        .into_iter()
        .map(|name| Ok((name.to_owned(), blobs.get(name)?)))
        .collect()
}

/// Show what changed in the review's files from version `from` to `to`.
///
/// `to` is the latest version when not given, and `from` the one before `to`.
pub(crate) fn diff(
    ctx: &Context<'_>,
    id: ReviewId,
    from: Option<u32>,
    to: Option<u32>,
) -> Result<Output, String> {
    let review = ctx.store.read(id)?;
    let to = version(id, &review, to)?;
    let from = version(id, &review, Some(from.unwrap_or_else(|| before(to))))?;

    print_changes(ctx, id, &review, [from, to], None)
}

/// Show what pass `pass` changed in the review's files: from the version it was
/// sent against to the one recorded when it ended.
pub(crate) fn diff_pass(ctx: &Context<'_>, id: ReviewId, pass: PassRef) -> Result<Output, String> {
    let review = ctx.store.read(id)?;
    let (number, span) = review.passes.span(pass).map_err(|why| why.message(id))?;

    print_changes(ctx, id, &review, span, Some(number))
}

/// Format the diff of `review` between versions `from` and `to` (or for `pass`
/// when specified), adding a note on stderr when no files changed.
fn print_changes(
    ctx: &Context<'_>,
    id: ReviewId,
    review: &Review,
    [from, to]: [u32; 2],
    pass: Option<u32>,
) -> Result<Output, String> {
    let changes = changes(&ctx.store.blobs(id), id, review, &review.paths, [from, to])?;

    if ctx.format.is_json() {
        return json(&changes);
    }

    let mut output = Output::from(render::diff(&changes));
    if changes.files.is_empty() {
        output.notes.push(match pass {
            Some(pass) => format!("Pass {pass} changed nothing in the files."),
            None => format!("Nothing changed from version {from} to version {to}."),
        });
    }

    Ok(output)
}

/// Default `from` version when comparing up to `to` (`max(1, to - 1)`).
pub(crate) fn before(to: u32) -> u32 {
    to.saturating_sub(1).max(1)
}

/// Compute the [`DiffView`] for `paths` in `review` between versions `from` and
/// `to`.
pub(crate) fn changes(
    blobs: &Blobs,
    id: ReviewId,
    review: &Review,
    paths: &[String],
    [from, to]: [u32; 2],
) -> Result<DiffView, String> {
    let mut files = vec![];
    for path in paths {
        let old = review
            .blob(from, path)
            .map(|name| blobs.get(name))
            .transpose()?;
        let new = review
            .blob(to, path)
            .map(|name| blobs.get(name))
            .transpose()?;
        files.extend(view::file_diff(path, old.as_deref(), new.as_deref()));
    }

    Ok(DiffView {
        review: id.to_string(),
        from,
        to,
        files,
    })
}

/// Close review `id`, warning if any comments remain open or in draft.
pub(crate) fn close(ctx: &Context<'_>, id: ReviewId) -> Result<Output, String> {
    let at = now();
    let closed = ctx.store.update(id, |review, _| {
        review.close(at).map_err(|refusal| refusal.message(id))
    })?;
    let review = ctx.store.read(id)?;

    let mut output = if ctx.format.is_json() {
        json(&view::summary(id, &review))?
    } else if closed {
        format!("Closed {id}.\n").into()
    } else {
        format!("{id} is closed already.\n").into()
    };
    if closed {
        output.warnings.extend(unsettled(&review));
    }

    Ok(output)
}

/// Format a warning listing any open or unsent draft comments when closing
/// `review`.
fn unsettled(review: &Review) -> Option<String> {
    let numbers = |status: Status| -> Vec<String> {
        review
            .comments
            .iter()
            .filter(|comment| comment.status() == status)
            .map(|comment| format!("#{}", comment.number))
            .collect()
    };
    let parts: Vec<String> = [
        (numbers(Status::Open), "still open"),
        (numbers(Status::Draft), "never sent"),
    ]
    .into_iter()
    .filter(|(numbers, _)| !numbers.is_empty())
    .map(|(numbers, state)| format!("{} {state}", numbers.join(", ")))
    .collect();

    (!parts.is_empty()).then(|| format!("Closed with comments {}.", parts.join(" and ")))
}
