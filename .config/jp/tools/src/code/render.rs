//! Plain-text formatting for `code_symbol` reports.
//!
//! Source snippets use `fs_grep_files` line markers: `:` for a reference or
//! implementation line, `-` for surrounding context, `=` for a line matching
//! `pattern`, and `--` between non-adjacent line ranges.
//! Each contiguous block is dedented by its common leading whitespace, and each
//! source line is emitted at most once per file.

use std::{
    collections::{BTreeMap, BTreeSet},
    mem,
    rc::Rc,
};

/// Context lines shown before and after a hit inside a partially rendered item.
const ITEM_CONTEXT: usize = 2;

/// Context lines shown before and after a hit outside any item.
const LOOSE_CONTEXT: usize = 1;

/// Maximum lines shown for a symbol's definition snippet.
const DEFINITION_LINES: usize = 60;

/// Maximum files listed in a count-only summary before collapsing the
/// remainder.
const COUNT_LINES: usize = 50;

/// Maximum non-test usage locations listed per member.
const MEMBER_PLACES: usize = 5;

/// Maximum signature lines retained from a hover response.
pub const SIGNATURE_LINES: usize = 20;

/// Maximum caller lines rendered across all depth levels.
pub const CALLER_LINES: usize = 100;

#[derive(Debug)]
pub struct Report {
    pub header: Header,
    pub definition: Option<Snippet>,
    pub implementations: Option<Hits>,
    pub references: Option<Hits>,
    pub callers: Option<Callers>,
    pub calls: Option<Calls>,
    pub members: Option<Members>,
}

/// Incoming call hierarchy for a function or method.
#[derive(Debug, Default)]
pub struct Callers {
    /// Diagnostic note when the target symbol is not a function or method.
    pub note: Option<String>,

    pub depth: usize,

    /// Flattened pre-order list of callers.
    pub shown: Vec<Caller>,

    /// Number of callers in test code omitted when `tests` is false.
    pub hidden_tests: usize,

    /// Whether traversal stopped at [`CALLER_LINES`].
    pub stopped: bool,
}

#[derive(Debug)]
pub struct Caller {
    /// Display label (e.g. `fn total`).
    pub label: String,

    /// Location of the first call site (`src/stream.rs:28`).
    pub location: String,

    pub calls: usize,

    /// Zero-based indentation depth (`0` for direct callers).
    pub level: usize,

    /// Whether this function already appeared earlier in `shown` with its own
    /// callers expanded.
    pub repeated: bool,
}

/// Outgoing calls from a function or method, ordered by first call site.
#[derive(Debug, Default)]
pub struct Calls {
    /// Diagnostic note when the target symbol is not a function or method.
    pub note: Option<String>,

    pub inside: Vec<Callee>,

    /// Names of callees defined outside the workspace.
    pub outside: Vec<String>,

    /// Workspace structs and enum variants constructed in the function body
    /// (`struct Pair`, `variant Shape::Circle`).
    pub named: Vec<String>,
}

#[derive(Debug)]
pub struct Callee {
    pub label: String,
    pub location: String,
    /// One-based line numbers of call sites within the caller.
    pub lines: Vec<usize>,
}

#[derive(Debug, Default)]
pub struct Header {
    /// Fully qualified symbol path (`fixture::stream::Stream::iter`).
    pub qualified: String,

    /// Definition location (`src/stream.rs:15`).
    pub location: String,

    pub signature: Vec<String>,

    /// First paragraph of the item's doc comment.
    pub doc: Vec<String>,
}

/// Source snippet for an item's definition, including doc comments.
#[derive(Debug)]
pub struct Snippet {
    pub path: String,

    /// Zero-based starting line number of `lines`.
    pub start: usize,

    pub lines: Vec<String>,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Tally {
    pub count: usize,
    pub files: usize,
}

/// Reference or implementation hits grouped by file and enclosing item.
#[derive(Debug, Default)]
pub struct Hits {
    /// Total hits across the workspace.
    pub total: usize,
    pub files: usize,

    /// Hits in readable files outside and inside test code.
    /// A file containing both increments `files` on both tallies.
    pub outside_tests: Tally,
    pub in_tests: Tally,

    /// Hits in files blocked by the access policy.
    pub withheld: usize,

    pub outside_workspace: usize,

    /// Whether `paths` restricted the displayed files.
    pub filtered: bool,

    /// Regex pattern restricting displayed hits to enclosing items with a
    /// matching line.
    pub pattern: Option<String>,

    pub shown: Vec<FileHits>,
}

/// Fields, variants, and methods of a type or trait, with reference counts for
/// each.
#[derive(Debug, Default)]
pub struct Members {
    /// Diagnostic note when the target cannot have members.
    pub note: Option<String>,

    pub members: Vec<Member>,

    /// Number of additional members truncated past the member limit.
    pub omitted: usize,
}

#[derive(Debug)]
pub struct Member {
    pub label: String,
    pub location: String,
    pub outside_tests: usize,
    pub in_tests: usize,

    /// References in files blocked by the access policy.
    pub withheld: usize,

    /// Non-test items referencing this member (`fn total (src/stream.rs:28)`).
    pub places: Vec<String>,
}

#[derive(Debug)]
pub struct FileHits {
    pub path: String,
    pub source: Rc<[String]>,
    pub groups: Vec<Group>,
}

/// Hits within a single enclosing item (or at file scope when `item` is
/// `None`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Group {
    pub item: Option<GroupItem>,

    /// Sorted, deduplicated zero-based line numbers containing hits.
    pub lines: Vec<usize>,

    /// Zero-based end line of the statement starting at each entry in `lines`.
    pub ends: Vec<usize>,

    /// Sorted zero-based line numbers in the enclosing item that match
    /// `pattern`.
    pub matches: Vec<usize>,

    /// Total hit count in this group (may exceed `lines.len()` when multiple
    /// hits appear on one line).
    pub count: usize,

    pub test: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupItem {
    pub label: String,

    /// Zero-based first line of the item's declaration (after doc comments and
    /// attributes).
    pub start: usize,

    /// Zero-based inclusive end line of the item.
    pub end: usize,
}

/// Divide `budget` across items proportionally by `weights` after reserving up
/// to `minimum` lines per item.
///
/// Any rounding remainder goes to the first item with the largest weight.
/// If all weights are zero or `budget` cannot cover `minimum` per item, splits
/// `budget` evenly.
#[must_use]
pub fn shares(budget: usize, weights: &[usize], minimum: usize) -> Vec<usize> {
    let count = weights.len();
    if count <= 1 {
        return vec![budget; count];
    }

    let floor = minimum.min(budget / count);
    let rest = budget - floor * count;
    let total = weights.iter().sum::<usize>();

    let mut shares = weights
        .iter()
        .map(|&weight| {
            floor
                + match total {
                    0 => rest / count,
                    total => rest * weight / total,
                }
        })
        .collect::<Vec<_>>();

    // The heaviest is the first of the largest weights.
    let heaviest = weights
        .iter()
        .enumerate()
        .rev()
        .max_by_key(|&(_, weight)| weight)
        .map_or(0, |(index, _)| index);
    shares[heaviest] += budget - shares.iter().sum::<usize>();

    shares
}

impl Report {
    /// Return the total number of hits this report will display (all
    /// implementations plus non-test references, and test references when
    /// `tests` is true).
    #[must_use]
    pub fn weight(&self, tests: bool) -> usize {
        let references = self.references.as_ref().map_or(0, |hits| {
            hits.outside_tests.count + if tests { hits.in_tests.count } else { 0 }
        });
        let implementations = self
            .implementations
            .as_ref()
            .map_or(0, |hits| hits.outside_tests.count + hits.in_tests.count);

        references + implementations
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Options {
    /// Line budget shared by the implementations and references sections.
    pub budget: usize,

    /// Whether to render test references and callers in full rather than as
    /// summary counts.
    pub tests: bool,
}

/// Render a symbol report as plain text.
///
/// The implementations and references sections share `options.budget`: when
/// both are present, implementations receive up to half the budget and
/// references receive the remainder.
/// Implementations in test code are always rendered in full after non-test
/// implementations.
#[must_use]
pub fn render(report: &Report, options: Options) -> String {
    let mut out = vec![];
    render_header(&mut out, &report.header);

    if let Some(snippet) = &report.definition {
        out.push(String::new());
        render_definition(&mut out, snippet);
    }

    let mut budget = options.budget;
    if let Some(implementations) = &report.implementations {
        out.push(String::new());
        let share = if report.references.is_some() {
            budget / 2
        } else {
            budget
        };
        let used = render_hits(&mut out, "Implementations", implementations, Options {
            budget: share,
            tests: true,
        });
        budget -= used;
    }

    if let Some(references) = &report.references {
        out.push(String::new());
        render_hits(&mut out, "References", references, Options {
            budget,
            tests: options.tests,
        });
    }

    if let Some(callers) = &report.callers {
        out.push(String::new());
        render_callers(&mut out, callers);
    }

    if let Some(calls) = &report.calls {
        out.push(String::new());
        render_calls(&mut out, calls);
    }

    if let Some(members) = &report.members {
        out.push(String::new());
        render_members(&mut out, members);
    }

    out.join("\n")
}

fn render_callers(out: &mut Vec<String>, callers: &Callers) {
    if let Some(note) = &callers.note {
        out.push(format!("Callers: {note}"));
        return;
    }

    if callers.shown.is_empty() && callers.hidden_tests == 0 {
        out.push("Callers: none in the workspace.".to_owned());
        return;
    }

    out.push(if callers.depth > 1 {
        format!("Callers, {} levels up:", callers.depth)
    } else {
        "Callers:".to_owned()
    });

    for caller in &callers.shown {
        let mut line = format!(
            "{}{} ({})",
            "  ".repeat(caller.level),
            caller.label,
            caller.location
        );
        if caller.calls > 1 {
            line.push_str(&format!(", {} calls", caller.calls));
        }
        if caller.repeated {
            line.push_str(", its callers are listed above");
        }
        out.push(line);
    }

    if callers.hidden_tests > 0 {
        let noun = if callers.hidden_tests == 1 {
            "test function"
        } else {
            "test functions"
        };
        out.push(format!(
            "In tests, {} {noun}. Pass `tests: true` to show them.",
            callers.hidden_tests
        ));
    }

    if callers.stopped {
        out.push(format!(
            "Stopped at {CALLER_LINES} callers; ask about one of them to see further up."
        ));
    }
}

fn render_calls(out: &mut Vec<String>, calls: &Calls) {
    if let Some(note) = &calls.note {
        out.push(format!("Calls: {note}"));
        return;
    }

    let total = calls.inside.len() + calls.outside.len();
    if total == 0 {
        out.push("Calls: none.".to_owned());
    } else {
        render_callees(out, calls, total);
    }

    if !calls.named.is_empty() {
        out.push(format!(
            "Types and variants it names: {}",
            calls.named.join(", ")
        ));
    }
}

fn render_callees(out: &mut Vec<String>, calls: &Calls, total: usize) {
    out.push(if calls.outside.is_empty() {
        format!("Calls: {total}.")
    } else {
        format!(
            "Calls: {total}, {} outside the workspace.",
            calls.outside.len()
        )
    });

    for callee in &calls.inside {
        let lines = callee
            .lines
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        out.push(format!(
            "{} ({}), at {lines}",
            callee.label, callee.location
        ));
    }

    if !calls.outside.is_empty() {
        out.push(format!(
            "Outside the workspace: {}",
            calls.outside.join(", ")
        ));
    }
}

fn render_header(out: &mut Vec<String>, header: &Header) {
    out.push(format!("{} ({})", header.qualified, header.location));
    out.extend(
        header
            .doc
            .iter()
            .map(|line| format!("/// {line}").trim_end().to_owned()),
    );
    out.extend(header.signature.iter().cloned());
}

fn render_definition(out: &mut Vec<String>, snippet: &Snippet) {
    let end = snippet.start + snippet.lines.len().saturating_sub(1);
    out.push(format!(
        "Definition ({}:{}-{}):",
        snippet.path,
        snippet.start + 1,
        end + 1
    ));

    let shown = &snippet.lines[..snippet.lines.len().min(DEFINITION_LINES)];
    let indent = common_indent(shown.iter().map(String::as_str));
    for (offset, text) in shown.iter().enumerate() {
        out.push(format!(
            "{}-{}",
            snippet.start + offset + 1,
            strip_indent(text, indent)
        ));
    }

    let hidden = snippet.lines.len() - shown.len();
    if hidden > 0 {
        out.push(format!("... {hidden} more lines, to line {}", end + 1));
    }
}

/// Render a `Hits` section under `title` and return the number of budget lines
/// consumed.
fn render_hits(out: &mut Vec<String>, title: &str, hits: &Hits, options: Options) -> usize {
    let mut summary = if hits.total == 0 {
        format!("{title}: none in the workspace.")
    } else {
        let split = match (hits.outside_tests, hits.in_tests) {
            (Tally { count: 0, .. }, Tally { count: 0, .. }) => String::new(),
            (_, Tally { count: 0, .. }) => ", none in tests".to_owned(),
            (Tally { count: 0, .. }, _) => ", all in tests".to_owned(),
            (outside, tests) => format!(
                ": {} in {} outside tests, {} in {} in tests",
                outside.count,
                files(outside.files),
                tests.count,
                files(tests.files)
            ),
        };
        format!("{title}: {} in {}{split}.", hits.total, files(hits.files))
    };

    if hits.filtered || hits.pattern.is_some() {
        let groups = hits
            .shown
            .iter()
            .flat_map(|file| &file.groups)
            .collect::<Vec<_>>();
        let count = groups.iter().map(|group| group.count).sum::<usize>();
        let place = if hits.pattern.is_some() {
            match groups.len() {
                1 => "1 item".to_owned(),
                items => format!("{items} items"),
            }
        } else {
            files(hits.shown.len())
        };

        summary.push_str(&format!(" {count} in {place}"));
        if hits.filtered {
            summary.push_str(" under the given paths");
        }
        if let Some(pattern) = &hits.pattern {
            summary.push_str(&format!(" where `{pattern}` also matches"));
        }
        summary.push('.');
    }
    if hits.withheld > 0 {
        summary.push_str(&format!(
            " {} in files this tool may not read.",
            hits.withheld
        ));
    }
    if hits.outside_workspace > 0 {
        summary.push_str(&format!(
            " {} more outside the workspace.",
            hits.outside_workspace
        ));
    }
    out.push(summary);

    let Plan {
        planned,
        over,
        collapsed: tests,
        used,
    } = plan(hits, options);

    let mut tests_headed = false;
    for file in &planned {
        if file.test && !tests_headed {
            tests_headed = true;
            out.push("In tests:".to_owned());
        }
        out.extend(file.render());
    }

    if !tests.is_empty() {
        let count = tests.iter().map(|(_, count)| count).sum::<usize>();
        out.push(format!(
            "In tests, {count} in {}. Pass `tests: true` to show them:",
            files(tests.len())
        ));
        push_counts(out, &tests);
    }

    if !over.is_empty() {
        out.push("Not shown, over the output budget:".to_owned());
        push_counts(out, &over);
    }

    used
}

fn render_members(out: &mut Vec<String>, members: &Members) {
    if let Some(note) = &members.note {
        out.push(format!("Members: {note}"));
        return;
    }

    if members.members.is_empty() {
        out.push("Members: none.".to_owned());
        return;
    }

    out.push(format!(
        "Members: {}. References to each, outside tests and in tests:",
        members.members.len() + members.omitted
    ));

    for member in &members.members {
        let mut counts = if member.outside_tests + member.in_tests == 0 {
            "no references".to_owned()
        } else {
            format!(
                "{} outside tests, {} in tests",
                member.outside_tests, member.in_tests
            )
        };
        if member.withheld > 0 {
            counts.push_str(&format!(
                ", {} in files this tool may not read",
                member.withheld
            ));
        }
        out.push(format!("{} ({}): {counts}", member.label, member.location));

        for place in member.places.iter().take(MEMBER_PLACES) {
            out.push(format!("  in {place}"));
        }
        if member.places.len() > MEMBER_PLACES {
            out.push(format!(
                "  and {} more places",
                member.places.len() - MEMBER_PLACES
            ));
        }
    }

    if members.omitted > 0 {
        out.push(format!("... and {} more members", members.omitted));
    }
}

/// Budget allocation result for a `Hits` section.
struct Plan<'a> {
    planned: Vec<PlannedFile<'a>>,

    /// Files excluded because even their single-line hits exceeded the budget.
    over: Vec<(&'a str, usize)>,

    /// Test files summarized by count only when `options.tests` is false.
    collapsed: Vec<(&'a str, usize)>,

    /// Budget lines consumed by `planned`.
    used: usize,
}

/// Allocate `options.budget` across `hits` in three passes:
///
/// 1. Include each file at `Form::Lines` (hit lines only), prioritizing
///    non-test files and choosing files with the fewest lines first so the
///    maximum number of files fit.
///    Files that still exceed the budget are moved to `over`.
/// 2. Upgrade groups from `Form::Lines` to the shorter of `Form::Whole` or
///    `Form::Context`.
/// 3. Upgrade remaining `Form::Context` groups to `Form::Whole`.
///
/// Passes 2 and 3 apply the smallest line-count increases first, prioritizing
/// non-test groups before test groups.
fn plan(hits: &Hits, options: Options) -> Plan<'_> {
    let mut remaining = options.budget;
    let mut planned = vec![];
    let mut over = vec![];
    let mut tests = vec![];

    for test in [false, true] {
        let mut candidates = vec![];
        for (order, file) in hits.shown.iter().enumerate() {
            let groups = file
                .groups
                .iter()
                .filter(|group| group.test == test)
                .collect::<Vec<_>>();
            if groups.is_empty() {
                continue;
            }

            let count = groups.iter().map(|group| group.count).sum::<usize>();
            if test && !options.tests {
                tests.push((file.path.as_str(), count));
                continue;
            }

            candidates.push(PlannedFile {
                file,
                forms: vec![Form::Lines; groups.len()],
                groups,
                count,
                test,
                order,
            });
        }

        // Fewest lines first, so the most files fit.
        candidates.sort_by_key(|candidate| candidate.render().len());
        let mut headed = false;
        for candidate in candidates {
            // The first file in tests brings the `In tests:` heading.
            let cost = candidate.render().len() + usize::from(test && !headed);
            if cost > remaining {
                over.push((
                    candidate.order,
                    candidate.file.path.as_str(),
                    candidate.count,
                ));
                continue;
            }

            remaining -= cost;
            headed |= test;
            planned.push(candidate);
        }
    }

    planned.sort_by_key(|file| (file.test, file.order));
    over.sort_unstable();

    // Lines to the shorter of the whole item and the context around them.
    let mut upgrades = vec![];
    for (file_index, file) in planned.iter().enumerate() {
        for (group_index, group) in file.groups.iter().enumerate() {
            let source = &file.file.source;
            let form = if whole_is_shortest(source, group) {
                Form::Whole
            } else {
                Form::Context
            };
            let added = form.render(source, group).len() - Form::Lines.render(source, group).len();
            upgrades.push((file.test, added, file_index, group_index, form));
        }
    }
    remaining = upgrade(&mut planned, upgrades, remaining);

    // Context to the whole item.
    let mut upgrades = vec![];
    for (file_index, file) in planned.iter().enumerate() {
        for (group_index, group) in file.groups.iter().enumerate() {
            if file.forms[group_index] != Form::Context || group.item.is_none() {
                continue;
            }

            let source = &file.file.source;
            let added =
                Form::Whole.render(source, group).len() - Form::Context.render(source, group).len();
            upgrades.push((file.test, added, file_index, group_index, Form::Whole));
        }
    }
    remaining = upgrade(&mut planned, upgrades, remaining);

    Plan {
        planned,
        over: over
            .into_iter()
            .map(|(_, path, count)| (path, count))
            .collect(),
        collapsed: tests,
        used: options.budget - remaining,
    }
}

/// Apply upgrades that fit within `remaining` budget, ordered by non-test first
/// and then by smallest line increase, returning the unused budget.
fn upgrade(
    planned: &mut [PlannedFile<'_>],
    mut upgrades: Vec<(bool, usize, usize, usize, Form)>,
    mut remaining: usize,
) -> usize {
    upgrades.sort_unstable_by_key(|&(test, added, file, group, _)| (test, added, file, group));
    for (_, added, file_index, group_index, form) in upgrades {
        if added <= remaining {
            remaining -= added;
            planned[file_index].forms[group_index] = form;
        }
    }

    remaining
}

/// Detail level for rendering a [`Group`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Form {
    /// Hit lines only, without enclosing item headers or surrounding context.
    Lines,

    /// Enclosing item header and context lines around each hit.
    Context,

    /// Enclosing item header and the full item body.
    Whole,
}

impl Form {
    /// Render `group` in isolation to measure its line cost during planning.
    fn render(self, source: &[String], group: &Group) -> Vec<String> {
        self.render_into(source, group, &marks(group), &mut BTreeSet::new())
    }

    /// Render `group`'s lines that are not yet in `seen`, inserting rendered
    /// line numbers into `seen`.
    fn render_into(
        self,
        source: &[String],
        group: &Group,
        marks: &BTreeMap<usize, char>,
        seen: &mut BTreeSet<usize>,
    ) -> Vec<String> {
        match self {
            Self::Lines => anchors(group)
                .0
                .into_iter()
                .filter(|&line| seen.insert(line))
                .map(|line| {
                    let text = source.get(line).map_or("", |text| text.trim());
                    let marker = marks.get(&line).copied().unwrap_or(':');
                    format!("{}{marker}{text}", line + 1)
                })
                .collect(),
            Self::Context => render_group_into(source, group, false, marks, seen),
            Self::Whole => render_group_into(source, group, true, marks, seen),
        }
    }
}

/// Build a map from zero-based line number to its marker character (`:` for
/// hits, `=` for `pattern` matches, with `:` taking precedence).
fn marks(group: &Group) -> BTreeMap<usize, char> {
    let mut marks = group
        .matches
        .iter()
        .map(|&line| (line, '='))
        .collect::<BTreeMap<_, _>>();
    marks.extend(group.lines.iter().map(|&line| (line, ':')));
    marks
}

/// Return all marked line numbers in `group` (`lines` and `matches`) paired
/// with their statement end lines.
fn anchors(group: &Group) -> (Vec<usize>, Vec<usize>) {
    let mut anchors = group
        .matches
        .iter()
        .map(|&line| (line, line))
        .collect::<BTreeMap<_, _>>();
    for (index, &line) in group.lines.iter().enumerate() {
        let end = group.ends.get(index).copied().unwrap_or(line).max(line);
        anchors.insert(line, end);
    }

    anchors.into_iter().unzip()
}

/// Planned rendering state for a single file (either non-test or test subset).
struct PlannedFile<'a> {
    file: &'a FileHits,
    groups: Vec<&'a Group>,
    forms: Vec<Form>,
    count: usize,
    test: bool,

    /// Original index in `hits.shown` used to restore file ordering.
    order: usize,
}

impl PlannedFile<'_> {
    /// Render the `path (count)` header followed by each group, deduplicating
    /// lines shared across nested items.
    fn render(&self) -> Vec<String> {
        let mut marked = BTreeMap::new();
        for group in &self.groups {
            for (line, mark) in marks(group) {
                let entry = marked.entry(line).or_insert(mark);
                if mark == ':' {
                    *entry = mark;
                }
            }
        }

        let mut seen = BTreeSet::new();
        let mut lines = vec![format!("{} ({})", self.file.path, self.count)];
        for (group, form) in self.groups.iter().zip(&self.forms) {
            lines.extend(form.render_into(&self.file.source, group, &marked, &mut seen));
        }

        lines
    }
}

/// Return whether rendering the full enclosing item takes no more lines than
/// rendering context windows around its hits.
fn whole_is_shortest(source: &[String], group: &Group) -> bool {
    group.item.is_some()
        && render_group(source, group, true).len() <= render_group(source, group, false).len()
}

/// Render a single group in isolation.
fn render_group(source: &[String], group: &Group, whole: bool) -> Vec<String> {
    render_group_into(source, group, whole, &marks(group), &mut BTreeSet::new())
}

/// Render `group`, skipping lines already present in `seen` and omitting the
/// group header if all of its lines were already rendered.
fn render_group_into(
    source: &[String],
    group: &Group,
    whole: bool,
    marks: &BTreeMap<usize, char>,
    seen: &mut BTreeSet<usize>,
) -> Vec<String> {
    let last = source.len().saturating_sub(1);
    let (anchors, ends) = anchors(group);

    let (label, spans) = match &group.item {
        Some(item) => {
            let end = item.end.min(last);
            let start = item.start.min(end);
            let label = format!("in {} ({}-{}):", item.label, start + 1, end + 1);

            let spans = if whole {
                vec![(start, end)]
            } else {
                spans(&anchors, &ends, ITEM_CONTEXT, start, end)
            };
            (Some(label), spans)
        }
        None => (None, spans(&anchors, &ends, LOOSE_CONTEXT, 0, last)),
    };

    // The stretches of lines not shown yet, split where a shown line was.
    let mut runs = vec![];
    for (start, end) in spans {
        let mut run = vec![];
        for line in start..=end {
            if seen.insert(line) {
                run.push(line);
            } else if !run.is_empty() {
                runs.push(mem::take(&mut run));
            }
        }
        if !run.is_empty() {
            runs.push(run);
        }
    }
    if runs.is_empty() {
        return vec![];
    }

    let indent = common_indent(
        runs.iter()
            .flatten()
            .filter_map(|&line| source.get(line))
            .map(String::as_str),
    );

    let mut lines = label.into_iter().collect::<Vec<_>>();
    for (index, run) in runs.iter().enumerate() {
        if index > 0 {
            lines.push("--".to_owned());
        }

        for &line in run {
            let text = source
                .get(line)
                .map_or("", |text| strip_indent(text, indent));
            let marker = marks.get(&line).copied().unwrap_or('-');
            lines.push(format!("{}{marker}{text}", line + 1).trim_end().to_owned());
        }
    }

    lines
}

/// Compute merged, inclusive line ranges within `low..=high` covering each line
/// in `hits` with `context` lines before and after and extending through the
/// corresponding statement end in `ends`.
fn spans(
    hits: &[usize],
    ends: &[usize],
    context: usize,
    low: usize,
    high: usize,
) -> Vec<(usize, usize)> {
    let mut spans: Vec<(usize, usize)> = vec![];

    for (index, &hit) in hits.iter().enumerate() {
        let statement = ends.get(index).copied().unwrap_or(hit);
        let start = hit.saturating_sub(context).max(low);
        let end = (hit + context).max(statement).min(high);
        if start > end {
            continue;
        }

        match spans.last_mut() {
            Some((_, previous)) if start <= *previous + 1 => *previous = (*previous).max(end),
            _ => spans.push((start, end)),
        }
    }

    spans
}

fn push_counts(out: &mut Vec<String>, counts: &[(&str, usize)]) {
    for (path, count) in counts.iter().take(COUNT_LINES) {
        out.push(format!("{path} ({count})"));
    }

    let rest = &counts[counts.len().min(COUNT_LINES)..];
    if !rest.is_empty() {
        let count = rest.iter().map(|(_, count)| count).sum::<usize>();
        out.push(format!("... and {count} more in {}", files(rest.len())));
    }
}

fn files(count: usize) -> String {
    if count == 1 {
        "1 file".to_owned()
    } else {
        format!("{count} files")
    }
}

/// Return the minimum leading whitespace byte count across non-blank lines.
fn common_indent<'a>(lines: impl Iterator<Item = &'a str>) -> usize {
    lines
        .filter(|line| !line.trim().is_empty())
        .map(|line| line.len() - line.trim_start().len())
        .min()
        .unwrap_or_default()
}

fn strip_indent(line: &str, indent: usize) -> &str {
    match line.get(..indent) {
        Some(prefix) if prefix.trim().is_empty() => &line[indent..],
        _ => line.trim_start(),
    }
}

pub fn cap(mut lines: Vec<String>, max: usize) -> Vec<String> {
    if lines.len() > max {
        lines.truncate(max);
        lines.push("...".to_owned());
    }

    lines
}

#[cfg(test)]
#[path = "render_tests.rs"]
mod tests;
