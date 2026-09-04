use std::{collections::HashSet, fmt::Write as _, num::NonZeroUsize, ops::Range};

use chrono::{DateTime, Utc};
use crossterm::style::Stylize as _;
use jp_conversation::ConversationId;
use jp_term::{
    osc::hyperlink,
    width::{
        display_width, prefix_end_for_width, suffix_start_for_width, truncate_to_width, wrap_ranges,
    },
};
use jp_workspace::ConversationHandle;
use rayon::prelude::*;
use serde_json::json;
use tracing::warn;

use crate::{
    cmd::{
        ConversationLoadRequest, Error, Output,
        conversation_id::FlagIds,
        label::{self, LabelSelector},
    },
    ctx::Ctx,
    output::print_json,
    shared::search::{
        ConcreteScope, Matcher, event_lines, event_scope, label_lines, resolve_ignore_case,
        title_for,
    },
};

/// Display columns always left for a hit's text, however wide the line prefix
/// grows.
const MIN_TEXT_WIDTH: usize = 20;

/// Marks an end of a hit's text that was cut to fit the available columns.
const ELLIPSIS: &str = "\u{2026}";

/// Display columns of what follows a match that the window keeps in view when
/// it slides right.
///
/// Enough for a few short words, so a match arrives with the start of its
/// sentence rather than flush against the right margin.
const WINDOW_TRAILING_CONTEXT: usize = 16;

/// Display columns the window will give up to open on a word boundary instead
/// of mid-word.
///
/// A stray fragment of a word reads as noise, and prose puts a boundary within
/// a few columns.
/// An unbroken run wider than this is more likely a path or an identifier,
/// where even a fragment tells the reader something, so the window keeps the
/// ragged edge rather than spending a third of itself on the search.
const WINDOW_SNAP_COLUMNS: usize = 12;

/// The kind field of a line-mode record whose line contains the pattern.
const MATCH_KIND: char = 'm';

/// The kind field of a line-mode record pulled in by `--context`.
const CONTEXT_KIND: char = 'c';

/// The turn field of a hit that isn't turn-scoped.
///
/// `..` is the whole conversation in the `--turn` selector `print` and
/// `compact` accept, so a title hit's coordinate stays usable as-is.
const WHOLE_CONVERSATION: &str = "..";

#[derive(Debug, Default, clap::Args)]
pub(crate) struct Grep {
    /// The search pattern.
    ///
    /// Multiple words are joined with single spaces, so quoting is optional:
    /// `jp c grep two words` searches for `two words`.
    /// A pattern that starts with `-` still needs `--` ahead of it.
    // `required` because a `Vec` positional is optional by default in clap, and
    // `num_args` only constrains values per occurrence. Without it a bare
    // `jp c grep` compiles the empty pattern, which matches every line of every
    // conversation.
    #[arg(value_name = "PATTERN", required = true, num_args = 1..)]
    pattern: Vec<String>,

    #[command(flatten)]
    target: FlagIds<true, true>,

    /// Match case-insensitively, overriding smart-case.
    #[arg(long, conflicts_with = "case_sensitive")]
    ignore_case: bool,

    /// Match case-sensitively, overriding smart-case.
    #[arg(long)]
    case_sensitive: bool,

    /// Number of context lines to show around each match.
    #[arg(long, default_value_t = 0)]
    context: usize,

    /// Sort conversations by a field.
    #[arg(long, value_enum, default_value_t)]
    sort: Sort,

    /// Reverse the sort order (newest/latest first).
    #[arg(long)]
    descending: bool,

    /// Treat the pattern as a regular expression instead of a literal.
    #[arg(long, short)]
    regex: bool,

    /// What to emit for each conversation that contains a match.
    ///
    /// - `hits`: the matching lines with their coordinates.
    /// - `ids`: the conversation ID only.
    /// - `count`: the conversation ID and its number of matching lines.
    /// - `text`: the matched and context lines with no coordinates.
    #[arg(long, value_enum, default_value_t)]
    output: OutputKind,

    /// Group hits under a per-conversation heading.
    ///
    /// On by default in the human format (`-F auto` on a terminal), off in the
    /// machine format.
    #[arg(long, conflicts_with = "no_heading")]
    heading: bool,

    /// Print one fully-qualified `ID:TURN:SCOPE:KIND:TEXT` line per hit instead
    /// of grouping them under headings.
    #[arg(long)]
    no_heading: bool,

    /// Wrap long lines instead of cutting them to fit.
    ///
    /// Wraps at the output width, so pair it with `--width` to pick a column:
    /// `jp c grep --width=100 --wrap PATTERN`.
    /// Has no effect when the output width is unknown, which is the case for a
    /// pipe with no `--width`.
    /// Groups hits under per-conversation headings even when piped, since a
    /// continuation row has no coordinate to carry.
    #[arg(long, conflicts_with = "no_heading")]
    wrap: bool,

    /// Stop after this many matching lines per conversation.
    ///
    /// Must be at least 1: a cap of zero would discard every match and report
    /// the run as finding nothing.
    #[arg(long, short = 'm')]
    max_matches: Option<NonZeroUsize>,

    /// Show at most this many conversations, in sort order.
    ///
    /// Must be at least 1: a cap of zero would discard real matches and report
    /// the run as finding nothing.
    #[arg(long)]
    limit: Option<NonZeroUsize>,

    /// Only search conversations carrying this label.
    ///
    /// `key=value` matches when the key holds that value, a bare `key` matches
    /// any value.
    /// Repeat the flag to require several; every selector must match.
    #[arg(long = "label", value_name = "KEY[=VALUE]")]
    labels: Vec<LabelSelector>,

    /// Restrict the search to specific parts of the conversation.
    ///
    /// Repeat the flag (`--scope user --scope assistant`) or comma-separate the
    /// values (`--scope user,assistant`).
    /// If omitted, every part is searched.
    /// Meta-scopes `chat` and `tool` expand to their concrete members.
    // One value per occurrence: a multi-value `--scope` would swallow the
    // positional pattern that follows it in every documented example.
    #[arg(long = "scope", short = 's', value_enum, value_delimiter = ',')]
    scopes: Vec<Scope>,
}

impl Grep {
    pub(crate) fn conversation_load_request(&self) -> ConversationLoadRequest {
        ConversationLoadRequest::explicit_or_none(&self.target)
    }

    #[expect(clippy::needless_pass_by_value)]
    pub(crate) fn run(self, ctx: &mut Ctx, handles: Vec<ConversationHandle>) -> Output {
        let explicit_case = match (self.ignore_case, self.case_sensitive) {
            (true, _) => Some(true),
            (_, true) => Some(false),
            _ => None,
        };
        // Rejoined with single spaces: the shell already split on whitespace, so
        // an unquoted multi-word pattern arrives as several arguments.
        let pattern = self.pattern.join(" ");
        let ignore_case = resolve_ignore_case(&pattern, explicit_case);

        // An unusable pattern is a failure, not an empty result: exit 2 so a
        // script can tell a broken pattern from a pattern that found nothing.
        let matcher = Matcher::new(&pattern, self.regex, ignore_case)
            .map_err(|e| (2, format!("invalid pattern: {e}")))?;

        let wanted = expand_scopes(&self.scopes);

        // If handles were provided, search only those. Otherwise search all.
        let mut ids: Vec<_> = if handles.is_empty() {
            ctx.workspace.conversations().map(|(id, _)| *id).collect()
        } else {
            handles.iter().map(ConversationHandle::id).collect()
        };

        // Narrow the candidate set before any searching: reading a
        // conversation's events costs far more than checking its metadata.
        if !self.labels.is_empty() {
            let matching: HashSet<_> = ctx
                .workspace
                .conversations()
                .filter(|(_, c)| label::matches(&c.labels, &self.labels))
                .map(|(id, _)| *id)
                .collect();
            ids.retain(|id| matching.contains(id));
        }

        self.sort_ids(&mut ids, ctx);

        // The global `--quiet` asks for no output, which leaves the exit status
        // as the whole answer: stop at the first match instead of collecting
        // every hit nobody will read.
        if ctx.term.args.quiet {
            // Ungated: the gate exists to stop paying the backtrack limit for
            // results that will be discarded, but here a single found match is
            // the whole answer and outlives any failure. Leaving it on would
            // make the status depend on whether a worker hit the poisoning
            // conversation before the matching one.
            let matcher = matcher.ungated();
            let matched = self.any_match(&ids, &matcher, &wanted, ctx);

            // A found match wins over a failure, as in `grep -q`: "if -q is
            // given and a line is selected, the exit status is 0 even if an
            // error occurred".
            if matched {
                return Ok(());
            }
            if let Some(error) = matcher.failure() {
                return Err((2, format!("pattern matching failed: {error}")).into());
            }

            // Marked expected for the same reason `render_empty` is: finding
            // nothing is a result, not a failure. This path bypasses
            // `render_empty` entirely, so it carries the marker itself.
            return Err(Error::from(1).expected());
        }

        let mut groups = self.collect_hits(&ids, &matcher, &wanted, ctx);

        // A pattern that failed part-way through leaves an unknown result, so it
        // is reported instead of the hits collected so far.
        if let Some(error) = matcher.failure() {
            return Err((2, format!("pattern matching failed: {error}")).into());
        }

        if let Some(limit) = self.limit {
            groups.truncate(limit.get());
        }

        if groups.is_empty() {
            return Err(Self::render_empty(ctx));
        }

        self.render(&groups, ctx);
        Ok(())
    }

    /// Whether any conversation contains a match.
    ///
    /// Stops as soon as one is found; the remaining conversations are never
    /// read.
    fn any_match(
        &self,
        ids: &[ConversationId],
        matcher: &Matcher,
        wanted: &HashSet<ConcreteScope>,
        ctx: &Ctx,
    ) -> bool {
        let needs_events = needs_events_for(wanted);

        ids.par_iter()
            .find_any(|&&id| {
                !self
                    .collect_hits_for_id(id, matcher, wanted, needs_events, Some(1), ctx)
                    .hits
                    .is_empty()
            })
            .is_some()
    }

    /// Report the absence of matches and produce the exit status for it.
    ///
    /// JSON consumers get a well-formed empty result; a terminal gets a short
    /// note on the chrome channel; a pipe gets nothing at all, matching `grep`.
    ///
    /// The status is always 1, marked expected: it is non-zero so a script can
    /// branch on it, but the run did what was asked, so failure-only output
    /// (the trace log location) stays quiet for a piped run.
    fn render_empty(ctx: &Ctx) -> Error {
        if ctx.printer.format().is_json() {
            print_json(&ctx.printer, &json!([]));
        } else if ctx.printer.pretty_printing_enabled() {
            ctx.printer.eprintln("No matches.".dim().to_string());
        }

        Error::from(1).expected()
    }

    fn collect_hits(
        &self,
        ids: &[ConversationId],
        matcher: &Matcher,
        wanted: &HashSet<ConcreteScope>,
        ctx: &Ctx,
    ) -> Vec<ConversationHits> {
        // Any scope other than `Title` is sourced from the event stream.
        // Skipping the event pass entirely when it can't contribute avoids a
        // sequential disk read per conversation.
        let needs_events = needs_events_for(wanted);

        // rayon's `collect` is order-preserving (unlike `reduce`), so the
        // already-sorted id order survives the parallel pass.
        ids.par_iter()
            .map(|&id| {
                self.collect_hits_for_id(
                    id,
                    matcher,
                    wanted,
                    needs_events,
                    self.max_matches.map(NonZeroUsize::get),
                    ctx,
                )
            })
            .filter(|group| !group.hits.is_empty())
            .collect()
    }

    /// Per-conversation hit collection.
    /// Pure function over `&Ctx`, so it is safe to invoke concurrently from a
    /// rayon worker.
    fn collect_hits_for_id(
        &self,
        id: ConversationId,
        matcher: &Matcher,
        wanted: &HashSet<ConcreteScope>,
        needs_events: bool,
        max_matches: Option<usize>,
        ctx: &Ctx,
    ) -> ConversationHits {
        let mut group = ConversationHits {
            id,
            title: None,
            turn_count: None,
            hits: Vec::new(),
        };

        let handle = match ctx.workspace.acquire_conversation(&id) {
            Ok(handle) => handle,
            Err(error) => {
                warn!(%id, %error, "Failed to load conversation");
                return group;
            }
        };

        group.title = title_for(ctx, &handle);

        // Counts down as matches are taken, so `--max-matches` applies across
        // the whole conversation rather than per scope.
        let mut budget = Budget::new(max_matches);

        if wanted.contains(&ConcreteScope::Title)
            && let Some(title) = &group.title
        {
            let lines: Vec<_> = title.lines().collect();
            collect_scope_hits(
                &mut group.hits,
                None,
                ConcreteScope::Title,
                None,
                &lines,
                matcher,
                self.context,
                &mut budget,
            );
        }

        if wanted.contains(&ConcreteScope::Label) {
            let labels = label_lines(ctx, &handle);
            let lines: Vec<&str> = labels.iter().map(String::as_str).collect();
            collect_scope_hits(
                &mut group.hits,
                None,
                ConcreteScope::Label,
                None,
                &lines,
                matcher,
                self.context,
                &mut budget,
            );
        }

        if !needs_events {
            return group;
        }

        let events = match ctx.workspace.events(&handle) {
            Ok(events) => events,
            Err(error) => {
                warn!(%id, %error, "Failed to load conversation events");
                return group;
            }
        };

        // Counted up front so `--max-matches` cutting the walk short below can't
        // understate it. Allocation-free, unlike `iter_turns`.
        group.turn_count = Some(events.turn_count());

        // `iter_events_by_turn` rather than `iter_turns`: the latter resolves and
        // clones a `PartialAppConfig` per event, which grep never reads.
        for (index, event) in events.iter_events_by_turn() {
            if budget.is_exhausted() {
                return group;
            }

            let Some(scope) = event_scope(&event.kind) else {
                continue;
            };
            if !wanted.contains(&scope) {
                continue;
            }

            let lines = event_lines(&event.kind);
            let line_refs: Vec<&str> = lines.iter().map(AsRef::as_ref).collect();

            collect_scope_hits(
                &mut group.hits,
                // 1-based to match the `--turn` selector and the headers `print`
                // renders.
                Some(index + 1),
                scope,
                Some(event.timestamp),
                &line_refs,
                matcher,
                self.context,
                &mut budget,
            );
        }

        group
    }

    fn render(&self, groups: &[ConversationHits], ctx: &Ctx) {
        match self.output {
            OutputKind::Ids => Self::render_ids(groups, ctx),
            OutputKind::Count => Self::render_count(groups, ctx),
            OutputKind::Text => Self::render_plain_text(groups, ctx),
            OutputKind::Hits if ctx.printer.format().is_json() => Self::render_json(groups, ctx),
            OutputKind::Hits => self.render_hits(groups, ctx),
        }
    }

    /// Whether hits are grouped under per-conversation headings.
    ///
    /// A terminal gets headings; a pipe gets one self-contained line per hit so
    /// that field-splitting tools see a fixed shape.
    /// `--wrap` forces headings: its continuation rows carry no coordinate, so
    /// wrapping and the one-record-per-line shape cannot both hold.
    fn heading_enabled(&self, pretty: bool) -> bool {
        if self.heading || self.wrap {
            true
        } else if self.no_heading {
            false
        } else {
            pretty
        }
    }

    fn render_hits(&self, groups: &[ConversationHits], ctx: &Ctx) {
        let pretty = ctx.printer.pretty_printing_enabled();
        let columns = ctx.term.width.map(usize::from);

        // Following `grep`, `--` group separators belong to context output. They
        // delimit blocks of surrounding lines; with no `--context` there are no
        // blocks, and a separator between two adjacent matches says nothing. In
        // line mode it would also break the promise that every emitted line is a
        // coordinate record.
        let separators = self.context > 0;

        let mut output = String::new();
        if self.heading_enabled(pretty) {
            for (index, group) in groups.iter().enumerate() {
                if index > 0 {
                    output.push('\n');
                }
                render_group_heading(&mut output, group, columns, pretty);
                render_group_hits(&mut output, group, columns, self.wrap, pretty, separators);
            }
        } else {
            for (index, group) in groups.iter().enumerate() {
                // Without headings a conversation boundary is just another gap
                // between groups of lines, so it gets the same `--` marker.
                // Never styled: see `write_group_break`.
                if separators && index > 0 {
                    write_group_break(&mut output, "", false);
                }
                render_group_lines(&mut output, group, columns, pretty, separators);
            }
        }

        ctx.printer.println_raw(output.trim_end_matches('\n'));
    }

    /// Print the matched and context lines with no coordinates and no
    /// separators, for piping content into another tool.
    fn render_plain_text(groups: &[ConversationHits], ctx: &Ctx) {
        // Verbatim, like every unbudgeted output path: trailing whitespace can
        // be the match itself, and machine output must not edit the text.
        let lines: Vec<&str> = groups
            .iter()
            .flat_map(|group| group.hits.iter())
            .map(|hit| hit.text.as_str())
            .collect();

        if ctx.printer.format().is_json() {
            print_json(&ctx.printer, &json!(lines));
            return;
        }

        ctx.printer.println_raw(lines.join("\n"));
    }

    fn render_json(groups: &[ConversationHits], ctx: &Ctx) {
        let entries: Vec<_> = groups
            .iter()
            .flat_map(|group| {
                group.hits.iter().map(move |hit| {
                    json!({
                        "id": group.id.to_string(),
                        "turn": hit.turn,
                        "scope": hit.scope.as_str(),
                        "timestamp": hit.timestamp,
                        "title": group.title,
                        // Emitted verbatim: `submatches` offsets index this
                        // string, so trimming it would shift them.
                        "text": hit.text,
                        "match": hit.is_match,
                        "submatches": submatches_json(hit),
                    })
                })
            })
            .collect();

        print_json(&ctx.printer, &json!(entries));
    }

    /// Print the ID of each conversation that contains a match, one per line (a
    /// JSON array under `-F json`), in sort order.
    fn render_ids(groups: &[ConversationHits], ctx: &Ctx) {
        let ids: Vec<String> = groups.iter().map(|group| group.id.to_string()).collect();

        if ctx.printer.format().is_json() {
            print_json(&ctx.printer, &json!(ids));
            return;
        }

        ctx.printer.println_raw(ids.join("\n"));
    }

    /// Print `ID:COUNT` per conversation, where the count is its number of
    /// matching lines.
    fn render_count(groups: &[ConversationHits], ctx: &Ctx) {
        if ctx.printer.format().is_json() {
            let entries: Vec<_> = groups
                .iter()
                .map(|group| json!({ "id": group.id.to_string(), "count": group.match_count() }))
                .collect();

            print_json(&ctx.printer, &json!(entries));
            return;
        }

        let lines: Vec<String> = groups
            .iter()
            .map(|group| format!("{}:{}", group.id, group.match_count()))
            .collect();

        ctx.printer.println_raw(lines.join("\n"));
    }

    fn sort_ids(&self, ids: &mut [ConversationId], ctx: &Ctx) {
        // Each key is computed once per conversation rather than once per
        // comparison. `activated` and `updated` reach into the workspace for
        // metadata, and doing that inside the comparator costs O(n log n)
        // handle acquisitions and lookups where O(n) is enough.
        let mut keyed: Vec<(Option<DateTime<Utc>>, ConversationId)> = ids
            .iter()
            .map(|id| (self.sort_key(*id, ctx), *id))
            .collect();

        keyed.sort_by(|(a, _), (b, _)| {
            let ord = a.cmp(b);
            if self.descending { ord.reverse() } else { ord }
        });

        for (slot, (_, id)) in ids.iter_mut().zip(keyed) {
            *slot = id;
        }
    }

    /// The timestamp `--sort` orders a conversation by.
    ///
    /// `None` sorts first ascending, which is where a conversation with no
    /// events lands under `updated`.
    fn sort_key(&self, id: ConversationId, ctx: &Ctx) -> Option<DateTime<Utc>> {
        let metadata = || {
            ctx.workspace
                .acquire_conversation(&id)
                .ok()
                .and_then(|handle| ctx.workspace.metadata(&handle).ok())
        };

        match self.sort {
            Sort::Created => Some(id.timestamp()),
            // A conversation whose metadata can't be read sorts as the epoch
            // rather than dropping out of the ordering.
            Sort::Activated => Some(metadata().map(|m| m.last_activated_at).unwrap_or_default()),
            Sort::Updated => metadata().and_then(|m| m.last_event_at),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, clap::ValueEnum)]
enum Sort {
    /// Sort by creation time (conversation ID).
    #[default]
    Created,

    /// Sort by last activation time.
    Activated,

    /// Sort by last event time.
    Updated,
}

/// What `--output` emits for each conversation that contains a match.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, clap::ValueEnum)]
#[clap(rename_all = "kebab-case")]
enum OutputKind {
    /// Matching lines with their `ID:TURN:SCOPE` coordinate.
    #[default]
    Hits,

    /// The conversation ID only.
    Ids,

    /// The conversation ID and its number of matching lines.
    Count,

    /// Matched and context lines with no coordinate.
    Text,
}

/// Parts of a conversation that can be restricted with `--scope`.
///
/// Meta-scopes (`all`, `chat`, `tool`) expand to one or more `ConcreteScope`s
/// at search time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, clap::ValueEnum)]
#[clap(rename_all = "kebab-case")]
enum Scope {
    /// Search every part (default when no `--scope` is given).
    All,

    /// Shorthand for `user`, `assistant`, `reasoning`, `structured`.
    Chat,

    /// Shorthand for `tool-call` and `tool-result`.
    Tool,

    /// The conversation title.
    Title,

    /// Conversation labels, each read as `key=value` or as a bare key.
    Label,

    /// User chat requests.
    User,

    /// Assistant chat responses (message text).
    Assistant,

    /// Assistant reasoning text.
    Reasoning,

    /// Structured assistant output.
    Structured,

    /// Tool call requests (name and arguments).
    ToolCall,

    /// Tool call results.
    ToolResult,

    /// Inquiry questions.
    Inquiry,
}

/// Whether the wanted scope set contains anything sourced from the event
/// stream.
///
/// `Title` and `Label` come from the conversation's metadata, so a search
/// restricted to them never touches the stream.
fn needs_events_for(wanted: &HashSet<ConcreteScope>) -> bool {
    wanted
        .iter()
        .any(|s| !matches!(s, ConcreteScope::Title | ConcreteScope::Label))
}

/// Expand a user-facing list of scopes to the concrete set the search uses.
///
/// An empty input (no `--scope` flag) behaves as `all`.
fn expand_scopes(scopes: &[Scope]) -> HashSet<ConcreteScope> {
    if scopes.is_empty() {
        return ConcreteScope::ALL.iter().copied().collect();
    }

    let mut out = HashSet::new();
    for scope in scopes {
        match scope {
            Scope::All => out.extend(ConcreteScope::ALL),
            Scope::Chat => {
                out.extend([
                    ConcreteScope::User,
                    ConcreteScope::Assistant,
                    ConcreteScope::Reasoning,
                    ConcreteScope::Structured,
                ]);
            }
            Scope::Tool => {
                out.extend([ConcreteScope::ToolCall, ConcreteScope::ToolResult]);
            }
            Scope::Title => _ = out.insert(ConcreteScope::Title),
            Scope::Label => _ = out.insert(ConcreteScope::Label),
            Scope::User => _ = out.insert(ConcreteScope::User),
            Scope::Assistant => _ = out.insert(ConcreteScope::Assistant),
            Scope::Reasoning => _ = out.insert(ConcreteScope::Reasoning),
            Scope::Structured => _ = out.insert(ConcreteScope::Structured),
            Scope::ToolCall => _ = out.insert(ConcreteScope::ToolCall),
            Scope::ToolResult => _ = out.insert(ConcreteScope::ToolResult),
            Scope::Inquiry => _ = out.insert(ConcreteScope::Inquiry),
        }
    }
    out
}

/// Every hit found in one conversation, with what a heading needs to describe
/// it.
struct ConversationHits {
    id: ConversationId,
    title: Option<String>,

    /// Total turns in the conversation, as a sense of its size.
    ///
    /// `None` when the event stream was never read — a search restricted to
    /// the title and labels skips it — so the figure is omitted rather than
    /// reported as zero.
    turn_count: Option<usize>,

    hits: Vec<Hit>,
}

impl ConversationHits {
    /// Number of matching lines, excluding the context lines around them.
    fn match_count(&self) -> usize {
        self.hits.iter().filter(|hit| hit.is_match).count()
    }
}

/// A single output line from a grep search.
struct Hit {
    /// The 1-based turn the line came from, or `None` for a conversation-scoped
    /// scope such as the title.
    turn: Option<usize>,

    /// Where in the conversation this line came from.
    scope: ConcreteScope,

    /// When the event carrying this line was recorded, or `None` for a
    /// conversation-scoped scope.
    timestamp: Option<DateTime<Utc>>,

    /// The line text.
    text: String,

    /// Byte ranges of the pattern matches within `text`.
    /// Empty for a context line.
    spans: Vec<Range<usize>>,

    /// If false, this is a "context" line.
    is_match: bool,

    /// Whether a `--` group separator should precede this line.
    group_break: bool,
}

impl Hit {
    /// The turn field of this hit's coordinate.
    fn turn_field(&self) -> String {
        self.turn
            .map_or_else(|| WHOLE_CONVERSATION.to_owned(), |turn| turn.to_string())
    }
}

/// How many more matches may still be taken.
struct Budget(Option<usize>);

impl Budget {
    /// A budget of `max` matches, or an unlimited one when `max` is `None`.
    const fn new(max: Option<usize>) -> Self {
        Self(max)
    }

    /// Whether no further matches may be taken.
    const fn is_exhausted(&self) -> bool {
        matches!(self.0, Some(0))
    }

    /// Reduce `indices` to the matches the budget still allows, and spend them.
    fn take(&mut self, indices: Vec<usize>) -> Vec<usize> {
        let Some(remaining) = self.0.as_mut() else {
            return indices;
        };

        let taken = indices.len().min(*remaining);
        *remaining -= taken;
        indices.into_iter().take(taken).collect()
    }
}

/// Run the match+context pipeline for a single scope source and append hits.
fn collect_scope_hits(
    hits: &mut Vec<Hit>,
    turn: Option<usize>,
    scope: ConcreteScope,
    timestamp: Option<DateTime<Utc>>,
    lines: &[&str],
    matcher: &Matcher,
    context: usize,
    budget: &mut Budget,
) {
    if lines.is_empty() {
        return;
    }

    let match_indices = budget.take(matching_lines(lines, matcher));
    if match_indices.is_empty() {
        return;
    }

    let block_start = hits.len();
    let ranges = context_ranges(&match_indices, context, lines.len());
    for (range_idx, (start, end)) in ranges.iter().enumerate() {
        for (i, line) in lines.iter().enumerate().skip(*start).take(end - start + 1) {
            let is_match = match_indices.contains(&i);
            hits.push(Hit {
                turn,
                scope,
                timestamp,
                spans: if is_match {
                    matcher.find_spans(line)
                } else {
                    vec![]
                },
                text: (*line).to_owned(),
                is_match,
                group_break: range_idx > 0 && i == *start,
            });
        }
    }

    mark_block_break(hits, block_start);
}

/// Return indices of lines that match the pattern.
fn matching_lines(lines: &[&str], matcher: &Matcher) -> Vec<usize> {
    lines
        .iter()
        .enumerate()
        .filter(|(_, line)| matcher.is_match(line))
        .map(|(i, _)| i)
        .collect()
}

/// Build merged, non-overlapping `(start, end)` ranges around each match index,
/// expanded by `ctx` lines in both directions, clamped to `[0, count)`.
fn context_ranges(indices: &[usize], ctx: usize, count: usize) -> Vec<(usize, usize)> {
    let mut ranges: Vec<(usize, usize)> = Vec::new();
    for &idx in indices {
        let start = idx.saturating_sub(ctx);
        let end = (idx + ctx).min(count - 1);
        if let Some(last) = ranges.last_mut() {
            // Merge with previous range if overlapping or adjacent.
            if start <= last.1 + 1 {
                last.1 = last.1.max(end);
                continue;
            }
        }

        ranges.push((start, end));
    }

    ranges
}

/// Mark the hit at `start` as needing a `--` separator before it.
///
/// `start` is where a block of hits began.
/// Nothing is marked when the block is empty or is the first one, so a
/// separator only ever lands *between* blocks.
fn mark_block_break(hits: &mut [Hit], start: usize) {
    if start > 0
        && let Some(hit) = hits.get_mut(start)
    {
        hit.group_break = true;
    }
}

/// Display columns available for a hit's text, given the width of the line
/// prefix.
///
/// `None` means no limit.
/// A prefix wide enough to fill the line on its own still leaves
/// [`MIN_TEXT_WIDTH`] columns.
fn text_budget(columns: Option<usize>, prefix_width: usize) -> Option<usize> {
    let columns = columns?;
    Some(columns.saturating_sub(prefix_width).max(MIN_TEXT_WIDTH))
}

/// How a hit's text is laid out against the available columns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Layout {
    /// One row carrying the text exactly as stored.
    ///
    /// What the `TEXT` field of a piped record promises, and the only option
    /// when no output width is known.
    Verbatim,

    /// One row of at most the given columns, windowed onto the text.
    Window(usize),

    /// As many rows of at most the given columns as the text needs.
    Wrap(usize),
}

impl Layout {
    /// The layout for hit rows carrying a prefix of `prefix_width` columns.
    ///
    /// Verbatim whenever no output width is known, whether or not wrapping was
    /// asked for: there is no column to wrap at.
    fn new(columns: Option<usize>, prefix_width: usize, wrap: bool) -> Self {
        let Some(budget) = text_budget(columns, prefix_width) else {
            return Self::Verbatim;
        };

        if wrap {
            Self::Wrap(budget)
        } else {
            Self::Window(budget)
        }
    }
}

/// One display row of a hit's text.
struct Row {
    /// The row as printed, including any [`ELLIPSIS`] markers.
    text: String,

    /// Byte range within [`Self::text`] carried over from the hit's text.
    ///
    /// Excludes the markers, so a span can be clipped against it.
    kept: Range<usize>,

    /// Byte offset in the hit's text that `kept.start` corresponds to.
    origin: usize,
}

impl Row {
    /// A row carrying `text` whole.
    fn whole(text: &str) -> Self {
        Self {
            kept: 0..text.len(),
            text: text.to_owned(),
            origin: 0,
        }
    }

    /// A row showing `text[range]`, with an [`ELLIPSIS`] marking each end that
    /// the range cut.
    fn windowed(text: &str, range: Range<usize>, cut_left: bool, cut_right: bool) -> Self {
        let mut out = String::new();
        if cut_left {
            out.push_str(ELLIPSIS);
        }

        let start = out.len();
        out.push_str(&text[range.clone()]);
        let end = out.len();

        if cut_right {
            out.push_str(ELLIPSIS);
        }

        Self {
            text: out,
            kept: start..end,
            origin: range.start,
        }
    }
}

/// Lay a hit's text out as the rows to print for it.
///
/// Always returns at least one row, so a hit is never dropped for having no
/// text.
fn layout_rows(text: &str, spans: &[Range<usize>], layout: Layout) -> Vec<Row> {
    match layout {
        Layout::Verbatim => vec![Row::whole(text)],
        // Trailing whitespace is trimmed only once a budget applies, so that
        // invisible columns don't spend it. It survives verbatim output, where
        // it can be the match itself (`--regex '\s+$'`).
        Layout::Window(budget) => vec![window_row(text.trim_end(), spans, budget)],
        Layout::Wrap(budget) => wrap_rows(text.trim_end(), budget),
    }
}

/// Every row of `text`, broken to fit `budget` columns.
///
/// Rows carry no [`ELLIPSIS`]: nothing is cut, so there is nothing to mark.
fn wrap_rows(text: &str, budget: usize) -> Vec<Row> {
    wrap_ranges(text, budget)
        .into_iter()
        .map(|range| Row::windowed(text, range, false, false))
        .collect()
}

/// One row showing as much of `text` as `budget` columns allow, positioned so
/// the first match stays visible.
///
/// The window sits at the start of the line while the first match fits there,
/// which leaves a short line untouched and a long one cut at the right.
/// A match past the budget slides the window right until the match and
/// [`WINDOW_TRAILING_CONTEXT`] columns of what follows it are in view.
/// Keeping the match in view wins over the trailing context, and a match too
/// wide to contain is shown from its start.
/// A slid window opens between words where it can; see [`WINDOW_SNAP_COLUMNS`].
/// Each end the window cut is marked with an [`ELLIPSIS`], counted against the
/// budget.
///
/// Only the first span is honored.
/// Later matches are visible when they fall in the same window and cut off when
/// they don't.
fn window_row(text: &str, spans: &[Range<usize>], budget: usize) -> Row {
    if display_width(text) <= budget {
        return Row::whole(text);
    }

    // One column for the trailing marker.
    let at_start = prefix_end_for_width(text, budget.saturating_sub(1));

    // `text` is trimmed, so a span can reach past it; that match lay entirely
    // in whitespace and slides the window to the end of the line.
    let first = spans
        .first()
        .map(|span| span.start.min(text.len())..span.end.min(text.len()));

    // A context line has no spans, and a match already inside the window needs
    // no sliding.
    let Some(first) = first.filter(|span| span.end > at_start) else {
        return Row::windowed(text, 0..at_start, false, true);
    };

    // Trailing context is capped so that it can never push the start of the
    // match back out of the window. `budget - 2` is the narrowest the window
    // gets, carrying a marker at each end.
    let headroom = budget.saturating_sub(2);
    let target = (display_width(&text[..first.end]) + WINDOW_TRAILING_CONTEXT)
        .min(display_width(&text[..first.start]) + headroom);

    let end = prefix_end_for_width(text, target);
    let cut_right = end < text.len();

    // One column for the leading marker, and another for the trailing one when
    // the window stops short of the end of the line.
    let reserved = 1 + usize::from(cut_right);
    let start = suffix_start_for_width(&text[..end], budget.saturating_sub(reserved));
    let start = snap_to_word_start(text, start, first.start);

    // A window that reaches the start of the line cut nothing there, whatever
    // the reserve assumed. A match at column 0 wider than the budget lands here.
    Row::windowed(text, start..end, start > 0, cut_right)
}

/// Advance `start` past the partial word it landed in, so the window opens
/// between words.
///
/// Searches at most [`WINDOW_SNAP_COLUMNS`] columns ahead and never advances
/// past `limit`, which is where the match begins.
/// `start` is returned unchanged when neither allows a boundary to be reached.
fn snap_to_word_start(text: &str, start: usize, limit: usize) -> usize {
    let horizon = (start + prefix_end_for_width(&text[start..], WINDOW_SNAP_COLUMNS)).min(limit);
    if horizon <= start {
        return start;
    }

    let Some(at) = text[start..horizon].find(char::is_whitespace) else {
        return start;
    };

    // Past the whole run, so the window opens on the next word rather than on
    // the space before it.
    let tail = &text[start + at..];
    (start + at + (tail.len() - tail.trim_start().len())).min(limit)
}

/// Style each matched span visible in `row`, leaving the rest unstyled.
///
/// `spans` are byte ranges into the hit's text.
/// Each is shifted onto the row by the row's origin and clipped to the part of
/// it that came from that text, so a span belonging to another row is dropped
/// and one running past the row's end is cut there.
/// A cut span's styling carries onto the trailing [`ELLIPSIS`], marking it as
/// "the match continues" rather than "the line continues".
/// A span reaching back past the origin is clipped to the row's left edge,
/// which is how a match crossing a wrap break stays styled on both rows.
/// The leading [`ELLIPSIS`] is never styled: that clip lands at the start of
/// the kept region, after the marker.
/// A span whose clipped end isn't a character boundary is skipped rather than
/// split.
fn highlight(row: &Row, spans: &[Range<usize>]) -> String {
    let text = row.text.as_str();
    let mut out = String::with_capacity(text.len());
    let mut cursor = 0;

    for span in spans {
        // A span ending at or before the origin belongs to an earlier row.
        if span.end <= row.origin {
            continue;
        }

        // One that began on an earlier row resumes at this row's left edge.
        let start = span.start.saturating_sub(row.origin) + row.kept.start;
        if start >= row.kept.end || start < cursor {
            continue;
        }

        // A span reaching past the kept region takes the trailing marker with
        // it, in one styled run rather than two adjacent ones. Rows without a
        // marker end at `kept.end` anyway, so this is a plain clip there.
        let reach = span.end - row.origin + row.kept.start;
        let end = if reach > row.kept.end {
            text.len()
        } else {
            reach
        };

        if !text.is_char_boundary(start) || !text.is_char_boundary(end) {
            continue;
        }

        out.push_str(&text[cursor..start]);
        let _ = write!(out, "{}", text[start..end].red().bold());
        cursor = end;
    }

    out.push_str(&text[cursor..]);
    out
}

/// Render one row of a hit's text, styled for a match or dimmed for a context
/// line.
fn styled_row(hit: &Hit, row: &Row, pretty: bool) -> String {
    if !pretty {
        return row.text.clone();
    }
    if hit.is_match {
        return highlight(row, &hit.spans);
    }

    row.text.as_str().dim().to_string()
}

/// Write the `--` separator that precedes a hit starting a new group.
///
/// `styled` dims the marker, which suits a heading-mode view.
/// Line mode must leave it bare whatever the format: the separator is the one
/// row in that stream that isn't a record, and a consumer is told to drop it by
/// matching `--` exactly, which styling bytes would defeat.
fn write_group_break(output: &mut String, indent: &str, styled: bool) {
    if styled {
        let _ = writeln!(output, "{indent}{}", "--".dim());
    } else {
        let _ = writeln!(output, "{indent}--");
    }
}

/// Write a conversation's heading: its ID, title, and match and turn counts.
fn render_group_heading(
    output: &mut String,
    group: &ConversationHits,
    columns: Option<usize>,
    pretty: bool,
) {
    let matches = group.match_count();
    let noun = if matches == 1 { "match" } else { "matches" };
    let stats = match group.turn_count {
        Some(turns) => {
            let turn_noun = if turns == 1 { "turn" } else { "turns" };
            format!("{matches} {noun} · {turns} {turn_noun}")
        }
        None => format!("{matches} {noun}"),
    };

    let id_str = group.id.to_string();
    let title = group.title.as_deref().unwrap_or("(no title)");

    // Two spaces on either side of the title, and the stats pushed to the right
    // margin when there is a margin to push them to.
    let fixed = display_width(&id_str) + display_width(&stats) + 4;
    let title = match columns {
        Some(width) => truncate_to_width(title, width.saturating_sub(fixed)),
        None => title.to_owned(),
    };
    let gap = columns.map_or(2, |width| {
        width
            .saturating_sub(fixed + display_width(&title))
            .saturating_add(2)
    });

    if pretty {
        let linked = hyperlink(
            format!("jp://show-events/{id_str}"),
            id_str.clone().magenta().to_string(),
        );
        let _ = writeln!(
            output,
            "{linked}  {}{:gap$}{}",
            title.bold(),
            "",
            stats.dim(),
            gap = gap
        );
    } else {
        let _ = writeln!(output, "{id_str}  {title}{:gap$}{stats}", "", gap = gap);
    }
}

/// Write a conversation's hits indented under its heading, as
/// `TURN:SCOPE:TEXT`.
fn render_group_hits(
    output: &mut String,
    group: &ConversationHits,
    columns: Option<usize>,
    wrap: bool,
    pretty: bool,
    separators: bool,
) {
    const INDENT: &str = "  ";

    // Both fields are right-aligned to the widest in the group, so the text
    // starts in the same column on every row. That alignment is what lets a
    // context row leave its coordinate blank and still line up.
    //
    // Padded by character count rather than display width: a turn is digits or
    // `..` and a scope name is from a fixed ASCII set, so the two agree.
    let turn_width = group
        .hits
        .iter()
        .map(|hit| display_width(&hit.turn_field()))
        .max()
        .unwrap_or(0);
    let scope_width = group
        .hits
        .iter()
        .map(|hit| display_width(hit.scope.as_str()))
        .max()
        .unwrap_or(0);

    // `INDENT`, both padded fields, and the two `:` separators.
    let prefix_width = INDENT.len() + turn_width + scope_width + 2;
    let layout = Layout::new(columns, prefix_width, wrap);

    for hit in &group.hits {
        if separators && hit.group_break {
            write_group_break(output, INDENT, pretty);
        }

        for (index, row) in layout_rows(&hit.text, &hit.spans, layout)
            .iter()
            .enumerate()
        {
            let text = styled_row(hit, row, pretty);

            // A context row's coordinate is its match's, by construction: every
            // hit in a block comes from one event, so the turn and scope are
            // identical. Printing it once per block instead of once per line
            // makes a visible coordinate the match marker on its own, rather
            // than a `:`-versus-`-` difference the eye has to hunt for down a
            // dense block. A row continuing the one above it is blank for the
            // same reason.
            if !hit.is_match || index > 0 {
                let _ = writeln!(output, "{blank:prefix_width$}{text}", blank = "");
                continue;
            }

            let turn = format!("{:>turn_width$}", hit.turn_field());
            let scope = format!("{:>scope_width$}", hit.scope.as_str());

            if pretty {
                let _ = writeln!(output, "{INDENT}{}:{}:{text}", turn.green(), scope.dim());
            } else {
                let _ = writeln!(output, "{INDENT}{turn}:{scope}:{text}");
            }
        }
    }
}

/// Write a conversation's hits as self-contained `ID:TURN:SCOPE:TEXT` lines.
///
/// `separators` enables the `--` markers between non-contiguous groups; without
/// them every line emitted is a coordinate record.
fn render_group_lines(
    output: &mut String,
    group: &ConversationHits,
    columns: Option<usize>,
    pretty: bool,
    separators: bool,
) {
    let id_str = group.id.to_string();
    let id_styled = id_str.clone().magenta().to_string();

    for hit in &group.hits {
        // Never styled: see `write_group_break`.
        if separators && hit.group_break {
            write_group_break(output, "", false);
        }

        let kind = if hit.is_match {
            MATCH_KIND
        } else {
            CONTEXT_KIND
        };
        let turn = hit.turn_field();
        let scope = hit.scope.as_str();

        // Four separators plus the single-character kind field.
        let prefix_width = display_width(&id_str) + display_width(&turn) + display_width(scope) + 5;
        // Never wrapped: line mode promises every emitted line is a coordinate
        // record, and a continuation row has no coordinate to carry. `--wrap`
        // is rejected alongside `--no-heading` for the same reason.
        let layout = Layout::new(columns, prefix_width, false);
        let rows = layout_rows(&hit.text, &hit.spans, layout);
        let text = styled_row(hit, &rows[0], pretty);

        if pretty {
            let _ = writeln!(
                output,
                "{id_styled}:{}:{}:{}:{text}",
                turn.green(),
                scope.dim(),
                kind.dim()
            );
        } else {
            let _ = writeln!(output, "{id_str}:{turn}:{scope}:{kind}:{text}");
        }
    }
}

/// The `submatches` array for a hit: the matched text and its byte offsets,
/// mirroring `rg --json`.
///
/// Offsets index the hit's `text` exactly as the JSON emits it, so a consumer
/// can slice one out of the other.
fn submatches_json(hit: &Hit) -> Vec<serde_json::Value> {
    hit.spans
        .iter()
        .filter_map(|span| {
            let text = hit.text.get(span.clone())?;
            Some(json!({ "match": text, "start": span.start, "end": span.end }))
        })
        .collect()
}

#[cfg(test)]
#[path = "grep_tests.rs"]
mod tests;
