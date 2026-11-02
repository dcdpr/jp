//! `jp wiki check`: hold every page to the format, and every link to what it
//! points at.
//!
//! Errors fail the check:
//!
//! - A page's title and required fields, its status and date, and `Summary`
//!   last in the block.
//! - Labels the vocabulary refuses, or that it lets one page carry and several
//!   do.
//! - A `Parent`, or an `.order.json` entry, naming no active page, and parents
//!   that loop.
//! - Links and mentions to documents that don't exist, `#fragment`s naming no
//!   heading, and headings with a custom anchor.
//! - In a Reference page, links into source that miss: no such file, a line
//!   range past its end, or a code-span label whose symbol isn't in the range.
//!   In a Snapshot page, any relative link into source, which `jp wiki pin`
//!   turns into a permalink.
//! - A permalink whose commit is missing or off the default branch, or whose
//!   file, lines, or symbol aren't there at that commit.
//! - Embeds that loop, and any embed in a Snapshot page.
//!
//! Warnings are advisory, and `--strict` makes them errors: an orphan page,
//! which nothing reaches, and a Reference page citing an RFD that was
//! superseded or abandoned.
//!
//! A check reads only what its access allows.
//! A link to a file it may not read, or to a document in a directory it may not
//! list, is reported as unchecked rather than as a problem: there is nothing
//! wrong with it that the check could see.

use std::{
    collections::{HashMap, HashSet},
    fs,
    sync::LazyLock,
};

use camino::{Utf8Path, Utf8PathBuf};
use chrono::NaiveDate;
use doc::{
    files,
    heading::headings,
    metadata::{self, Header},
    references::names_stem,
};
use jp_label::Vocabulary;
use jp_tool::Capability;
use regex::Regex;
use serde::Serialize;

use crate::{
    access::Access,
    embed::{self, Expander, Problem},
    git::{self, DEFAULT_BRANCH},
    library::{Document, Library, Lookup},
    links::{self, Link, Named, has_scheme, line_range, resolve, split_fragment, symbol},
    options::{Options, Paths},
    page::{Page, PageId, Status, WikiKind},
    pin::permalink,
    store::Wiki,
};

/// A custom heading anchor, `{#turn}`, at the end of a heading line.
static CUSTOM_ANCHOR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\{#[^}]*\}\s*$").expect("valid regex"));

/// The fields every page carries.
const REQUIRED: [&str; 4] = ["Status", "Authors", "Date", "Summary"];

/// The wiki's front page, inside the wiki directory: the site's entry point,
/// which is no page itself but reaches the pages it links to.
const FRONT_PAGE: &str = "index.md";

/// One problem, where it is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct Finding {
    /// The file, relative to the workspace root.
    pub path: String,

    /// The 1-based line, when the problem is on one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<usize>,

    pub message: String,
}

/// What a check found.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub(crate) struct Report {
    /// How many pages were read.
    pub pages: usize,

    pub errors: Vec<Finding>,
    pub warnings: Vec<Finding>,

    /// What couldn't be checked, because the check's access doesn't let it read
    /// the file it would have to.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub unchecked: Vec<Finding>,
}

/// What a check reads besides the pages themselves.
pub(crate) struct Context<'a> {
    pub paths: &'a Paths,
    pub options: &'a Options,
    pub wiki: &'a Wiki,
    pub library: &'a Library,

    /// The web address permalinks name the repository by, when there is one.
    pub repository: Option<&'a str>,

    /// What the check may read.
    pub access: &'a Access,
}

/// Check every page, and what holds them together.
#[must_use]
pub(crate) fn check(context: &Context<'_>) -> Report {
    let mut checker = Checker::new(context);
    for page in &context.wiki.pages {
        checker.page(page);
    }
    checker.wiki();

    let mut report = checker.report;
    report.pages = context.wiki.pages.len();
    report
}

/// Check one page on its own: everything [`check`] holds a page to, but not
/// what concerns the wiki as a whole.
///
/// What an edit is held to before it is kept.
/// The report carries the page's errors and what couldn't be checked, and no
/// warnings: those concern the wiki as a whole.
#[must_use]
pub(crate) fn check_page(context: &Context<'_>, page: &Page) -> Report {
    let mut checker = Checker::new(context);
    checker.page(page);

    checker.report
}

/// A check in progress.
struct Checker<'a> {
    context: &'a Context<'a>,

    /// The vocabulary, or why it couldn't be read.
    vocabulary: Result<Vocabulary, String>,

    /// The heading anchors of each markdown file read so far; `None` for one
    /// that couldn't be read.
    anchors: HashMap<Utf8PathBuf, Option<HashSet<String>>>,

    report: Report,
}

impl<'a> Checker<'a> {
    fn new(context: &'a Context<'a>) -> Self {
        Self {
            context,
            vocabulary: context.options.vocabulary(),
            anchors: HashMap::new(),
            report: Report::default(),
        }
    }

    fn error(&mut self, path: &Utf8Path, line: Option<usize>, message: impl Into<String>) {
        let path = self.context.paths.show(path);
        self.report.errors.push(Finding {
            path,
            line,
            message: message.into(),
        });
    }

    fn warn(&mut self, path: &Utf8Path, line: Option<usize>, message: impl Into<String>) {
        let path = self.context.paths.show(path);
        self.report.warnings.push(Finding {
            path,
            line,
            message: message.into(),
        });
    }

    fn unchecked(&mut self, path: &Utf8Path, line: Option<usize>, message: impl Into<String>) {
        let path = self.context.paths.show(path);
        self.report.unchecked.push(Finding {
            path,
            line,
            message: message.into(),
        });
    }

    /// Everything one page is held to.
    fn page(&mut self, page: &Page) {
        self.format(page);
        self.relations(page);
        self.labels(page);
        self.headings(page);
        self.links(page);
        self.mentions(page);
        self.embeds(page);
    }

    /// The title, the required fields, and their values.
    fn format(&mut self, page: &Page) {
        let path = &page.path;
        if page.title.is_none() {
            self.error(
                path,
                Some(1),
                "The page doesn't open with a `# Title` heading.",
            );
        }

        let lines: Vec<&str> = page.source.lines().collect();
        let fields = Header::of(&lines).fields();
        for key in REQUIRED {
            let present = fields
                .iter()
                .any(|field| field.key.eq_ignore_ascii_case(key) && !field.value.is_empty());
            if !present {
                self.error(path, None, format!("The page has no `{key}`."));
            }
        }

        if let Some(status) = &page.status
            && let Err(error) = status.parse::<Status>()
        {
            self.error(path, None, error);
        }
        if let Some(date) = &page.date
            && NaiveDate::parse_from_str(date, "%Y-%m-%d").is_err()
        {
            self.error(path, None, format!("`{date}` is not a `YYYY-MM-DD` date."));
        }

        let last = fields.last().map(|field| field.key.as_str());
        if page.summary.is_some() && !last.is_some_and(|key| key.eq_ignore_ascii_case("Summary")) {
            self.error(path, None, "`Summary` is not the last field of the block.");
        }
    }

    /// `Parent` and `Superseded by`.
    ///
    /// An archived page keeps whatever parent it had, so only an active page's
    /// is held to naming an active page.
    fn relations(&mut self, page: &Page) {
        let context = self.context;
        let path = &page.path;

        if let Some(parent) = page.parent.as_ref().filter(|_| !page.is_archived()) {
            match parent.parse::<PageId>() {
                Err(_) => self.error(
                    path,
                    None,
                    format!("`Parent` names `{parent}`, not a page id."),
                ),
                Ok(id) if id == page.id => self.error(path, None, "The page is its own parent."),
                Ok(id) => match context.wiki.pages.iter().find(|other| other.id == id) {
                    // A page that couldn't be read exists, and whether it is
                    // archived is unknown.
                    None if context.wiki.exists(id) => self.unchecked(
                        path,
                        None,
                        format!("`Parent` names {id}, which can't be read to see if it is active."),
                    ),
                    None => self.error(
                        path,
                        None,
                        format!("`Parent` names {id}, and no page has it."),
                    ),
                    Some(other) if other.is_archived() => self.error(
                        path,
                        None,
                        format!("`Parent` names {id}, which is archived; name an active page."),
                    ),
                    Some(_) => {}
                },
            }
        }

        let Some(superseded) = &page.superseded_by else {
            return;
        };
        if !page.is_archived() {
            self.error(path, None, "`Superseded by` belongs on an archived page.");
        }
        match links::mentions(superseded).first() {
            Some(mention) if mention.named == Named::Wiki => {
                let exists = mention
                    .id
                    .parse::<PageId>()
                    .is_ok_and(|id| context.wiki.exists(id));
                if !exists {
                    self.error(
                        path,
                        None,
                        format!("`Superseded by` names {}, and no page has it.", mention.id),
                    );
                }
            }
            Some(mention) if mention.named == Named::Rfd => {
                match context.library.rfd(&mention.id) {
                    Lookup::Found(_) => {}
                    Lookup::Missing => self.error(
                        path,
                        None,
                        format!(
                            "`Superseded by` names RFD {}, and there is no such RFD.",
                            mention.id
                        ),
                    ),
                    Lookup::Unknown => self.unchecked(
                        path,
                        None,
                        format!(
                            "`Superseded by` names RFD {}, in a directory that can't be listed.",
                            mention.id
                        ),
                    ),
                }
            }
            _ => self.error(
                path,
                None,
                format!(
                    "`Superseded by` holds `{superseded}`; name a page, `W-0a1b2c3`, or an RFD, \
                     `RFD 042`."
                ),
            ),
        }
    }

    /// Labels the vocabulary refuses.
    fn labels(&mut self, page: &Page) {
        let vocabulary = match &self.vocabulary {
            Ok(vocabulary) => vocabulary.clone(),
            Err(_) => return,
        };

        if let Err(mut rejected) = vocabulary.resolve_against(&page.labels, &page.label_set()) {
            rejected.addable.clear();
            rejected.open_keys.clear();
            self.error(&page.path, None, rejected.to_string());
        }
    }

    /// Headings carrying a custom anchor, which a forge and a plain reader show
    /// as text.
    fn headings(&mut self, page: &Page) {
        let lines: Vec<&str> = page.source.lines().collect();
        for heading in headings(&page.source) {
            let Some(line) = lines.get(heading.line) else {
                continue;
            };
            if CUSTOM_ANCHOR.is_match(line) {
                self.error(
                    &page.path,
                    Some(heading.line + 1),
                    "A heading carries a custom anchor; the site computes every heading's anchor \
                     from its text.",
                );
            }
        }
    }

    /// Every link and image.
    fn links(&mut self, page: &Page) {
        let context = self.context;
        let dir = page.path.parent().unwrap_or(&context.paths.root);
        for link in links::links(&page.source) {
            let line = Some(link.line + 1);

            if has_scheme(&link.url) {
                if let Some(repository) = context.repository
                    && let Some(permalink) = permalink(&link.url, repository)
                {
                    self.permalink(page, &link, &permalink);
                }
                continue;
            }
            if link.url.starts_with('/') {
                // A route on the site, which only the site's build can check.
                continue;
            }

            let (path, fragment) = split_fragment(&link.url);
            let fragment = fragment.filter(|fragment| !fragment.is_empty());
            if path.is_empty() {
                if let Some(fragment) = fragment {
                    self.anchor(page, &page.path, fragment, line, &link.url);
                }
                continue;
            }

            let target = resolve(dir, path);
            self.relative(page, &link, &target, fragment);
        }
    }

    /// A relative link from `page` to `target`.
    fn relative(&mut self, page: &Page, link: &Link, target: &Utf8Path, fragment: Option<&str>) {
        let context = self.context;
        let paths = context.paths;
        let line = Some(link.line + 1);
        if !target.starts_with(&paths.root) {
            self.error(
                &page.path,
                line,
                format!("`{}` points outside the workspace.", link.url),
            );
            return;
        }

        let source = !target.starts_with(paths.docs());
        if source {
            match page.status() {
                Some(Status::Snapshot) => {
                    self.error(
                        &page.path,
                        line,
                        format!(
                            "`{}` cites source by a relative link, which follows the work tree; \
                             `jp wiki pin {}` turns it into a permalink.",
                            link.url, page.id
                        ),
                    );
                    return;
                }
                Some(Status::Archived) => return,
                _ => {}
            }
        }

        if !target.exists() {
            self.error(
                &page.path,
                line,
                format!("`{}`: there is no such file.", link.url),
            );
            return;
        }

        if target.extension() == Some("md") {
            if let Some(fragment) = fragment {
                self.anchor(page, target, fragment, line, &link.url);
            }
            return;
        }
        if !source || !target.is_file() {
            return;
        }
        if let Err(refusal) = context.access.check(Capability::Read, target) {
            self.unchecked(
                &page.path,
                line,
                format!("`{}` wasn't checked: {refusal}.", link.url),
            );
            return;
        }

        match fs::read_to_string(target) {
            Ok(text) => {
                let lines: Vec<&str> = text.lines().collect();
                if let Some(problem) = lines_problem(&lines, fragment, link.code_label.as_deref()) {
                    self.error(&page.path, line, format!("`{}`: {problem}", link.url));
                }
            }
            Err(error) => {
                self.error(&page.path, line, format!("`{}`: {error}.", link.url));
            }
        }
    }

    /// A permalink: its commit, and the file, lines, and symbol it names there.
    fn permalink(&mut self, page: &Page, link: &Link, permalink: &crate::pin::Permalink) {
        let context = self.context;
        let root = &context.paths.root;
        let line = Some(link.line + 1);
        let commit = &permalink.commit;
        let short = &commit[..commit.len().min(12)];

        if !git::has_commit(root, commit) {
            self.error(
                &page.path,
                line,
                format!(
                    "`{}`: commit {short} is missing here; a shallow clone needs the full history.",
                    link.url
                ),
            );
            return;
        }
        match git::is_ancestor(root, commit, DEFAULT_BRANCH) {
            Ok(true) => {}
            Ok(false) => {
                self.error(
                    &page.path,
                    line,
                    format!(
                        "`{}`: commit {short} is not on the default branch; a commit that only a \
                         branch holds goes when it is squash-merged.",
                        link.url
                    ),
                );
                return;
            }
            Err(error) => {
                self.error(&page.path, line, format!("`{}`: {error}.", link.url));
                return;
            }
        }

        let object = format!("{commit}:{}", permalink.path);
        let Some(kind) = git::object_type(root, &object) else {
            self.error(
                &page.path,
                line,
                format!(
                    "`{}`: {} is not there at {short}.",
                    link.url, permalink.path
                ),
            );
            return;
        };

        let fragment = permalink.fragment.as_deref().filter(|f| !f.is_empty());
        if kind != "blob" {
            if fragment.is_some() {
                self.error(
                    &page.path,
                    line,
                    format!("`{}`: a directory has no lines.", link.url),
                );
            }
            return;
        }
        if fragment.is_none() && link.code_label.is_none() {
            return;
        }
        // The commit's copy is the file's contents all the same, so it is read
        // only where the file in the work tree may be.
        if let Err(refusal) = self.readable_in_repository(&permalink.path) {
            self.unchecked(
                &page.path,
                line,
                format!("`{}` wasn't checked: {refusal}.", link.url),
            );
            return;
        }
        let text = git::blob(root, &object).unwrap_or_default();
        let lines: Vec<&str> = text.lines().collect();
        if let Some(problem) = lines_problem(&lines, fragment, link.code_label.as_deref()) {
            self.error(&page.path, line, format!("`{}`: {problem}", link.url));
        }
    }

    /// Whether the check's access may read `path`, a file's path from the top
    /// of the repository, in the work tree.
    fn readable_in_repository(&self, path: &str) -> Result<(), String> {
        let context = self.context;
        if !context.access.is_restricted() {
            return Ok(());
        }

        let root = &context.paths.root;
        let local = git::prefix(root)
            .and_then(|prefix| {
                path.strip_prefix(prefix.as_str())
                    .map(|rest| root.join(rest))
            })
            .ok_or_else(|| {
                format!("{path} is outside the workspace, which is all a tool reaches")
            })?;

        context.access.check(Capability::Read, &local)
    }

    /// A `#fragment` on a link from `page` to the markdown file `target`.
    fn anchor(
        &mut self,
        page: &Page,
        target: &Utf8Path,
        fragment: &str,
        line: Option<usize>,
        url: &str,
    ) {
        if let Err(refusal) = self.context.access.check(Capability::Read, target) {
            self.unchecked(
                &page.path,
                line,
                format!("`{url}` wasn't checked: {refusal}."),
            );
            return;
        }

        let known = self
            .anchors
            .entry(target.to_path_buf())
            .or_insert_with(|| {
                let text = fs::read_to_string(target).ok()?;
                Some(
                    headings(&text)
                        .into_iter()
                        .map(|heading| heading.slug)
                        .collect(),
                )
            })
            .as_ref()
            .is_none_or(|anchors| anchors.contains(fragment));

        if !known {
            self.error(
                &page.path,
                line,
                format!("`{url}`: `#{fragment}` names no heading there."),
            );
        }
    }

    /// Documents the page names by id, outside its metadata block, which
    /// [`Checker::relations`] holds to its own rules.
    fn mentions(&mut self, page: &Page) {
        let context = self.context;
        let lines: Vec<&str> = page.source.lines().collect();
        let block = metadata::block(&lines);
        let library = context.library;

        for mention in links::mentions(&page.source) {
            if block.contains(&mention.line) {
                continue;
            }
            let line = Some(mention.line + 1);
            let target = match mention.named {
                Named::Wiki => {
                    let path = mention
                        .id
                        .parse::<PageId>()
                        .ok()
                        .and_then(|id| context.wiki.path_of(id));
                    match path {
                        Some(path) => Lookup::Found(path),
                        None => Lookup::Missing,
                    }
                }
                Named::Rfd => found_path(library.rfd(&mention.id)),
                Named::Ticket => found_path(library.ticket(&mention.id)),
            };
            let name = match mention.named {
                Named::Rfd => format!("RFD {}", mention.id),
                _ => mention.id.clone(),
            };

            match (target, &mention.fragment) {
                (Lookup::Missing, _) => {
                    self.error(&page.path, line, format!("{name} names no document."));
                }
                (Lookup::Unknown, _) => self.unchecked(
                    &page.path,
                    line,
                    format!("{name} wasn't checked: its directory can't be listed."),
                ),
                (Lookup::Found(target), Some(fragment)) => {
                    let url = format!("{name}#{fragment}");
                    let target = target.to_path_buf();
                    self.anchor(page, &target, fragment, line, &url);
                }
                (Lookup::Found(_), None) => {}
            }
        }
    }

    /// Embeds in a Snapshot page, and embeds that loop.
    fn embeds(&mut self, page: &Page) {
        let context = self.context;
        let paths = context.paths;
        let dir = page.path.parent().unwrap_or(&paths.root);
        if page.status() == Some(Status::Snapshot) {
            for found in embed::embeds(&page.source, dir) {
                self.error(
                    &page.path,
                    Some(found.link.line + 1),
                    "A Snapshot page doesn't embed, since an embed shows the source as it is now; \
                     copy the excerpt and cite it.",
                );
            }
            return;
        }

        let mut expander = Expander::new(&paths.root, &paths.wiki, context.library, context.access);
        expander.expand(&page.source, &page.path, None);
        for problem in expander.problems {
            match problem {
                Problem::Loop(chain) => self.error(
                    &page.path,
                    None,
                    format!("Embeds loop: {}.", chain.join(" embeds ")),
                ),
                Problem::Denied { url, refusal } => self.unchecked(
                    &page.path,
                    None,
                    format!("The embed `{url}` wasn't checked: {refusal}."),
                ),
                // An embed that doesn't resolve is a link that doesn't, which
                // the link checks report.
                Problem::Unresolved(_) => {}
            }
        }
    }

    /// What holds the pages together: the epoch, ids, parents, the order file,
    /// labels one page carries, orphans, and superseded citations.
    fn wiki(&mut self) {
        let context = self.context;
        let dir = &context.paths.wiki;

        match files::read_epoch::<WikiKind>(dir) {
            Ok(Some(_)) => {}
            Ok(None) if context.wiki.pages.is_empty() => {}
            Ok(None) => self.error(
                &dir.join(doc::epoch::EPOCH_FILE),
                None,
                "The epoch file is missing, so no new page can be created; restore it from \
                 version control.",
            ),
            Err(error) => self.error(dir, None, error.to_string()),
        }
        if let Err(error) = &self.vocabulary {
            let error = error.clone();
            self.error(dir, None, error);
        }

        self.duplicates();
        self.loops();
        self.order();
        self.shared_labels();
        self.orphans();
        self.superseded_citations();
    }

    /// Ids more than one file claims.
    fn duplicates(&mut self) {
        let context = self.context;
        let pages = &context.wiki.pages;
        let mut seen: HashSet<PageId> = HashSet::new();
        for page in pages {
            if !seen.insert(page.id) {
                continue;
            }
            let claiming: Vec<String> = pages
                .iter()
                .filter(|other| other.id == page.id)
                .map(|other| context.paths.show(&other.path))
                .collect();
            if claiming.len() > 1 {
                self.error(
                    &page.path,
                    None,
                    format!(
                        "{} is claimed by more than one file: {}.",
                        page.id,
                        claiming.join(", ")
                    ),
                );
            }
        }
    }

    /// Parents that loop back to the page, reported once, on the page with the
    /// lowest id in the loop.
    fn loops(&mut self) {
        let context = self.context;
        let pages = &context.wiki.pages;
        let parents: HashMap<PageId, PageId> = pages
            .iter()
            .filter_map(|page| Some((page.id, page.parent_id()?)))
            .collect();

        for page in pages {
            let mut chain = vec![page.id];
            let mut at = page.id;
            while let Some(parent) = parents.get(&at) {
                if *parent == page.id {
                    if chain.iter().all(|member| page.id <= *member) {
                        chain.push(page.id);
                        let names: Vec<String> = chain.iter().map(ToString::to_string).collect();
                        self.error(
                            &page.path,
                            None,
                            format!("Parents loop: {}.", names.join(" under ")),
                        );
                    }
                    break;
                }
                if chain.contains(parent) {
                    break;
                }
                chain.push(*parent);
                at = *parent;
            }
        }
    }

    /// Entries in the order file naming no active page.
    fn order(&mut self) {
        let context = self.context;
        let file = context.paths.wiki.join(crate::order::ORDER_FILE);
        for (list, entry) in context.wiki.order.entries() {
            let id = entry.parse::<PageId>().ok();
            let page = id.and_then(|id| context.wiki.pages.iter().find(|page| page.id == id));
            match page {
                None if id.is_some_and(|id| context.wiki.exists(id)) => {}
                None => self.error(
                    &file,
                    None,
                    format!("`{list}` names `{entry}`, and no page has it."),
                ),
                Some(page) if page.is_archived() => self.error(
                    &file,
                    None,
                    format!("`{list}` names {}, which is archived.", page.id),
                ),
                Some(_) => {}
            }
        }
    }

    /// Labels the vocabulary lets one page carry, on several.
    fn shared_labels(&mut self) {
        let Ok(vocabulary) = self.vocabulary.clone() else {
            return;
        };
        let context = self.context;
        let pages = &context.wiki.pages;
        let mut reported = HashSet::new();
        for (index, page) in pages.iter().enumerate() {
            for other in &pages[index + 1..] {
                for clash in vocabulary.clashes(&page.label_set(), &other.label_set()) {
                    let what = clash.to_string();
                    if reported.insert((what.clone(), page.id, other.id)) {
                        self.error(
                            &other.path,
                            None,
                            format!(
                                "`{what}` is on {} too, and the vocabulary lets one page carry it.",
                                page.id
                            ),
                        );
                    }
                }
            }
        }
    }

    /// Active pages nothing reaches: no parent, no link from a page, RFD,
    /// ticket, or the wiki's front page, and no RFD named under the relevant
    /// key.
    fn orphans(&mut self) {
        let context = self.context;
        let front = context.paths.wiki.join(FRONT_PAGE);
        let front_source = context
            .access
            .reads(&front)
            .then(|| fs::read_to_string(&front).ok())
            .flatten();

        let documents = context
            .library
            .rfds
            .values()
            .chain(context.library.tickets.values())
            .filter_map(|document| Some((document.path.as_path(), document.source.as_deref()?)));
        let others: Vec<(&Utf8Path, &str)> = context
            .wiki
            .pages
            .iter()
            .map(|page| (page.path.as_path(), page.source.as_str()))
            .chain(documents)
            .chain(
                front_source
                    .as_deref()
                    .map(|source| (front.as_path(), source)),
            )
            .collect();

        for page in &context.wiki.pages {
            if page.is_archived() || page.parent.is_some() {
                continue;
            }
            if page.label_set().contains_key(&context.options.relevant_key) {
                continue;
            }

            let id = page.id.to_string();
            let stem = page.path.file_stem().unwrap_or_default();
            let reached = others.iter().any(|(path, source)| {
                *path != page.path && (links::names(source, &id) || names_stem(source, stem))
            });
            if !reached {
                self.warn(
                    &page.path,
                    None,
                    format!(
                        "{id} is an orphan: it has no parent, no page, RFD, ticket, or the wiki's \
                         front page links to it, and it names no RFD under `{}`.",
                        context.options.relevant_key
                    ),
                );
            }
        }
    }

    /// Reference pages citing an RFD that was superseded or abandoned.
    fn superseded_citations(&mut self) {
        let context = self.context;
        for page in &context.wiki.pages {
            if page.status() != Some(Status::Reference) {
                continue;
            }

            let dir = page.path.parent().unwrap_or(&context.paths.root);
            let mut cited: Vec<String> = links::mentions(&page.source)
                .into_iter()
                .filter(|mention| mention.named == Named::Rfd)
                .map(|mention| mention.id)
                .collect();
            for link in links::links(&page.source) {
                let (path, _) = split_fragment(&link.url);
                if path.is_empty() || has_scheme(&link.url) {
                    continue;
                }
                let target = resolve(dir, path);
                cited.extend(
                    context
                        .library
                        .rfds
                        .iter()
                        .filter(|(_, rfd)| rfd.path == target)
                        .map(|(id, _)| id.clone()),
                );
            }
            cited.sort();
            cited.dedup();

            for id in cited {
                let status = context
                    .library
                    .rfds
                    .get(&id)
                    .and_then(|rfd| rfd.status.clone());
                if let Some(status) =
                    status.filter(|s| matches!(s.as_str(), "Superseded" | "Abandoned"))
                {
                    self.warn(
                        &page.path,
                        None,
                        format!("The page cites RFD {id}, which is {status}."),
                    );
                }
            }
        }
    }
}

/// The path a lookup found, or why it found none.
fn found_path(lookup: Lookup<&Document>) -> Lookup<&Utf8Path> {
    match lookup {
        Lookup::Found(document) => Lookup::Found(document.path.as_path()),
        Lookup::Missing => Lookup::Missing,
        Lookup::Unknown => Lookup::Unknown,
    }
}

/// What is wrong with a link to the file holding `lines`, naming `fragment`,
/// and guarded by the code-span label `label`, if anything.
///
/// Without a fragment, the label's symbol has to be somewhere in the file.
fn lines_problem(lines: &[&str], fragment: Option<&str>, label: Option<&str>) -> Option<String> {
    let symbol = label.and_then(symbol);
    let Some(fragment) = fragment else {
        let symbol = symbol?;
        return (!lines.iter().any(|line| line.contains(symbol)))
            .then(|| format!("`{symbol}` is not in the file."));
    };

    let Some((start, end)) = line_range(fragment) else {
        return Some(format!(
            "`#{fragment}` is not a line range; write `#L12` or `#L12-L24`."
        ));
    };
    if start == 0 || start > end || end > lines.len() {
        return Some(format!(
            "lines {start} to {end} are past the file, which has {}.",
            lines.len()
        ));
    }

    let symbol = symbol?;
    if lines[start - 1..end]
        .iter()
        .any(|line| line.contains(symbol))
    {
        return None;
    }

    Some(match lines.iter().position(|line| line.contains(symbol)) {
        Some(moved) => format!(
            "`{symbol}` is not on lines {start} to {end}; it is on line {} now.",
            moved + 1
        ),
        None => format!("`{symbol}` is not in the file."),
    })
}

#[cfg(test)]
#[path = "check_tests.rs"]
mod tests;
