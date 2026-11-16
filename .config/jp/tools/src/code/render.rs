//! Plain-text rendering of what the language server said about a symbol.
//!
//! Source lines use the `fs_grep_files` convention: a line number followed by
//! `:` is a hit, followed by `-` is context, and `--` separates non-adjacent
//! stretches of one file.
//! A line number followed by `=` matches the `pattern` the hits were narrowed
//! by.
//! Each stretch is dedented by its common indentation, and no line of a file is
//! shown twice.

use std::{
    collections::{BTreeMap, BTreeSet},
    mem,
    rc::Rc,
};

/// Context lines around a hit inside an item not shown whole.
const ITEM_CONTEXT: usize = 2;

/// Context lines around a hit outside any item.
const LOOSE_CONTEXT: usize = 1;

/// The most lines of a definition shown.
const DEFINITION_LINES: usize = 60;

/// The most files listed by count alone before the rest are summed up.
const COUNT_LINES: usize = 50;

/// The most places outside tests listed under one member.
const MEMBER_PLACES: usize = 5;

/// The most signature lines taken from a hover.
pub const SIGNATURE_LINES: usize = 20;

/// The most callers listed, across every level.
pub const CALLER_LINES: usize = 100;

/// Everything shown about one symbol.
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

/// The functions that call a function, and the functions that call those, to
/// `depth` levels.
#[derive(Debug, Default)]
pub struct Callers {
    /// Why there are none to list, when the symbol is not a function.
    pub note: Option<String>,

    pub depth: usize,

    /// Each caller after the caller it calls, deeper levels indented.
    pub shown: Vec<Caller>,

    /// Test functions among the callers, counted but not listed.
    pub hidden_tests: usize,

    /// Whether listing stopped at [`CALLER_LINES`].
    pub stopped: bool,
}

#[derive(Debug)]
pub struct Caller {
    /// `fn total`.
    pub label: String,

    /// Where its first call is: `src/stream.rs:28`.
    pub location: String,

    pub calls: usize,

    /// How far up it is: 0 calls the symbol itself.
    pub level: usize,

    /// Whether it is listed with its own callers higher up.
    pub repeated: bool,
}

/// The functions a function calls, in the order it first calls them.
#[derive(Debug, Default)]
pub struct Calls {
    /// Why there are none to list, when the symbol is not a function.
    pub note: Option<String>,

    pub inside: Vec<Callee>,

    /// The names of functions outside the workspace that it calls.
    pub outside: Vec<String>,

    /// The workspace structs and variants it names, which the server lists with
    /// its calls: `struct Pair`, `variant Shape::Circle`.
    pub named: Vec<String>,
}

#[derive(Debug)]
pub struct Callee {
    /// `fn middle`.
    pub label: String,

    /// Where it is defined: `src/lib.rs:5`.
    pub location: String,

    /// The one-based lines of the calls.
    pub lines: Vec<usize>,
}

#[derive(Debug, Default)]
pub struct Header {
    /// The symbol's path: `fixture::stream::Stream::iter`.
    pub qualified: String,

    /// Where it is defined: `src/stream.rs:15`.
    pub location: String,

    pub signature: Vec<String>,

    /// The first paragraph of its doc comment.
    pub doc: Vec<String>,
}

/// The source of the symbol's item, doc comments included.
#[derive(Debug)]
pub struct Snippet {
    pub path: String,

    /// The zero-based line `lines` starts at.
    pub start: usize,

    pub lines: Vec<String>,
}

/// Locations and the files they are in.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Tally {
    pub count: usize,
    pub files: usize,
}

/// Locations the server returned for a symbol, references or implementations,
/// grouped by file and enclosing item.
#[derive(Debug, Default)]
pub struct Hits {
    /// Locations in the workspace, wherever they are.
    pub total: usize,
    pub files: usize,

    /// Locations in files the tool may read, outside test code and in it.
    /// A file with both counts toward both.
    pub outside_tests: Tally,
    pub in_tests: Tally,

    /// Locations in files the tool may not read, counted but not shown.
    pub withheld: usize,

    pub outside_workspace: usize,

    /// Whether `paths` narrowed what is shown.
    pub filtered: bool,

    /// The `pattern` that narrowed what is shown to the items it matches in.
    pub pattern: Option<String>,

    /// The files to show, in order.
    pub shown: Vec<FileHits>,
}

/// The fields, variants and methods of a type or trait, each with its
/// references.
#[derive(Debug, Default)]
pub struct Members {
    /// Why there are none to list, when the symbol is not a type or trait.
    pub note: Option<String>,

    pub members: Vec<Member>,

    /// Members past the most listed.
    pub omitted: usize,
}

#[derive(Debug)]
pub struct Member {
    /// `field color`, `fn iter`.
    pub label: String,

    /// Where it is defined: `src/stream.rs:9`.
    pub location: String,

    pub outside_tests: usize,
    pub in_tests: usize,

    /// References in files the tool may not read.
    pub withheld: usize,

    /// The items outside tests that refer to it: `fn total (src/stream.rs:28)`.
    pub places: Vec<String>,
}

#[derive(Debug)]
pub struct FileHits {
    pub path: String,
    pub source: Rc<[String]>,
    pub groups: Vec<Group>,
}

/// The references inside one enclosing item, or outside any item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Group {
    pub item: Option<GroupItem>,

    /// Zero-based lines with a reference, sorted and without repeats.
    pub lines: Vec<usize>,

    /// The last line of the statement each reference starts on, aligned with
    /// `lines` and never before its own line.
    pub ends: Vec<usize>,

    /// Zero-based lines in the item that match `pattern`, sorted.
    pub matches: Vec<usize>,

    /// References, which can exceed the lines when one line holds several.
    pub count: usize,

    pub test: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupItem {
    /// `fn Stream::iter`.
    pub label: String,

    /// The first line of the item's code, after doc comments and attributes.
    /// Zero-based.
    pub start: usize,

    /// Zero-based and inclusive.
    pub end: usize,
}

/// Split `budget` across items by `weights`: `minimum` each first, the rest in
/// proportion to each weight.
///
/// The rounding remainder goes to the heaviest item.
/// Without weights, or with a budget too small for `minimum` each, the split is
/// even.
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
    /// How many locations the report shows: implementations, and references
    /// outside tests plus those in tests when `tests` asks for them.
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
    /// The most lines the references section takes, counts aside.
    pub budget: usize,

    /// Show test references in full rather than as counts.
    pub tests: bool,
}

/// Render a report.
///
/// The implementations and references sections share `options.budget`: when
/// both are shown, implementations are planned against half of it and
/// references get what they leave.
/// Implementations in test code are always shown, after the others, since the
/// implementing type is the answer whichever code it sits in.
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

/// One section of locations, titled `title`: a summary line, then each file as
/// [`plan`] decides.
///
/// Returns the lines of `options.budget` the files took.
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

/// What [`plan`] decided for one section.
struct Plan<'a> {
    /// The files to show, in output order.
    planned: Vec<PlannedFile<'a>>,

    /// Files over the budget, with their counts.
    over: Vec<(&'a str, usize)>,

    /// Test files collapsed to counts.
    collapsed: Vec<(&'a str, usize)>,

    /// The lines of the budget the planned files take.
    used: usize,
}

/// Decide what of each file fits `options.budget`, in three passes:
///
/// 1. Every reference gets its own line: those outside tests first, then those
///    in tests when they are shown.
///    When the lines alone do not fit, the files with the fewest are kept and
///    the rest are counted as over the budget.
/// 2. Each reference's line grows to the shorter of its whole enclosing item
///    and the context around it.
/// 3. What is left shows items whole.
///
/// Passes 2 and 3 take the cheapest change first, those outside tests before
/// those in tests.
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

/// Apply each upgrade that fits `remaining`, cheapest first, those outside
/// tests before those in tests.
///
/// Returns what is left of `remaining`.
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

/// How one group of references is shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Form {
    /// The referencing lines alone, without the enclosing item or context.
    Lines,

    /// The enclosing item's label and the lines around each reference.
    Context,

    /// The enclosing item's label and the whole item.
    Whole,
}

impl Form {
    /// The group on its own, as the planner prices it.
    fn render(self, source: &[String], group: &Group) -> Vec<String> {
        self.render_into(source, group, &marks(group), &mut BTreeSet::new())
    }

    /// The group's lines not in `seen`, which takes them, each marked as
    /// `marks` says or as context.
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

/// The lines a group marks: `:` for a reference, `=` for a line matching the
/// pattern.
/// A reference that also matches is a reference.
fn marks(group: &Group) -> BTreeMap<usize, char> {
    let mut marks = group
        .matches
        .iter()
        .map(|&line| (line, '='))
        .collect::<BTreeMap<_, _>>();
    marks.extend(group.lines.iter().map(|&line| (line, ':')));
    marks
}

/// Every line a group marks, sorted, with the last line of the statement each
/// starts: its own line for a pattern match.
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

/// One shown file: which of its groups are in it and how each is shown.
struct PlannedFile<'a> {
    file: &'a FileHits,
    groups: Vec<&'a Group>,

    /// Per group, how it is shown.
    forms: Vec<Form>,

    count: usize,
    test: bool,

    /// The file's place in the hits, which the output keeps.
    order: usize,
}

impl PlannedFile<'_> {
    /// A `path (count)` line, then each group.
    ///
    /// A line an earlier group showed is not shown again, and a line any group
    /// marks keeps its mark under whichever group shows it.
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

/// Whether showing the group's whole item takes no more lines than the context
/// around each reference.
fn whole_is_shortest(source: &[String], group: &Group) -> bool {
    group.item.is_some()
        && render_group(source, group, true).len() <= render_group(source, group, false).len()
}

/// One group: its whole enclosing item when `whole` is set and it has one,
/// otherwise the lines around each marked line.
fn render_group(source: &[String], group: &Group, whole: bool) -> Vec<String> {
    render_group_into(source, group, whole, &marks(group), &mut BTreeSet::new())
}

/// [`render_group`], leaving out the lines in `seen` and adding the rest to it.
///
/// A group with nothing left to show renders as nothing, its label included.
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

/// The stretches of lines within `low..=high` that show each of `hits`, which
/// is sorted, with `context` lines around it, and on to the end of its
/// statement in `ends`, merged where they touch.
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

/// The smallest leading whitespace across the non-blank lines.
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
