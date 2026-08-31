//! Parser for macOS `sample(1)` call-graph output.
//!
//! `sample` writes an indented tree where each line has the form:
//!
//! ```text
//!     + ! 1234 _RNvCsh5KVlJR8TJf_6jp_cli3run  (in jp) + 6924  [0x10537dae0]
//! ```
//!
//! - leading whitespace + decorative `+`/`!`/`:` characters track tree depth
//! - the first integer on the data line is the sample count for that frame
//! - everything between the count and `(in <image>)` is the symbol, which may
//!   be Rust v0 mangled (`_R...`)
//!
//! We slice the file into one tree per thread, collapse it into a flat
//! per-frame structure, and demangle Rust symbols on the way out.

use rustc_demangle::demangle;

/// A single frame in a thread's call tree, with the sample count attributed to
/// it and its descendants.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Frame {
    /// Depth in the tree, 0 being the thread root.
    pub depth: usize,

    /// Sample count attached to this frame.
    pub samples: u64,

    /// Demangled symbol (or the raw symbol if demangling didn't apply).
    pub symbol: String,
}

/// One thread's call tree plus its header line.
#[derive(Debug, Clone)]
pub(crate) struct Thread {
    /// The header line preserved as-is (`Thread_12345`, plus any name).
    pub header: String,

    /// Frames in top-down order.
    /// Depth is recovered from the original indentation.
    pub frames: Vec<Frame>,
}

impl Thread {
    /// Aggregate sample counts by demangled symbol across this thread.
    ///
    /// Counts are *inclusive* (a frame plus everything it called), so a caller
    /// and its callees both count the same samples.
    /// Good for "how much time was spent under X", useless for "which code is
    /// hot" — use [`Self::self_samples`] for that.
    pub(crate) fn aggregate_by_symbol(&self) -> Vec<(String, u64)> {
        let mut map: std::collections::BTreeMap<String, u64> = std::collections::BTreeMap::new();
        for frame in &self.frames {
            *map.entry(frame.symbol.clone()).or_default() += frame.samples;
        }
        let mut vec: Vec<(String, u64)> = map.into_iter().collect();
        vec.sort_by_key(|entry| std::cmp::Reverse(entry.1));
        vec
    }

    /// Samples attributed to each frame *itself*, excluding its callees.
    ///
    /// `sample(1)` reports inclusive counts, so self time is a frame's count
    /// minus the sum of its immediate children's.
    /// Assumes frames are in depth-first preorder, which is how the parser
    /// emits them.
    ///
    /// Returned in the same order as [`Self::frames`].
    pub(crate) fn self_samples(&self) -> Vec<u64> {
        let mut selves: Vec<u64> = self.frames.iter().map(|f| f.samples).collect();

        // Ancestors still open at this point in the preorder walk, as
        // (depth, index), innermost last.
        let mut open: Vec<(usize, usize)> = Vec::new();

        for (index, frame) in self.frames.iter().enumerate() {
            while open.last().is_some_and(|&(depth, _)| depth >= frame.depth) {
                open.pop();
            }
            // Only an immediate parent absorbs the child's count. A depth jump
            // (malformed input) leaves the samples unattributed rather than
            // charging them to a grandparent.
            if let Some(&(depth, parent)) = open.last()
                && depth + 1 == frame.depth
            {
                selves[parent] = selves[parent].saturating_sub(frame.samples);
            }
            open.push((frame.depth, index));
        }

        selves
    }
}

/// Aggregate self time by symbol across every thread.
///
/// Returns `(symbol, self_samples)` sorted descending.
/// This is the "which code is actually hot" lens: for a thread-pool workload
/// the main thread is parked and every interesting frame lives on a worker.
pub(crate) fn self_samples_by_symbol(threads: &[Thread]) -> Vec<(String, u64)> {
    let mut map: std::collections::BTreeMap<String, u64> = std::collections::BTreeMap::new();

    for thread in threads {
        for (frame, self_samples) in thread.frames.iter().zip(thread.self_samples()) {
            if self_samples == 0 {
                continue;
            }
            *map.entry(frame.symbol.clone()).or_default() += self_samples;
        }
    }

    let mut vec: Vec<(String, u64)> = map.into_iter().collect();
    vec.sort_by_key(|entry| std::cmp::Reverse(entry.1));
    vec
}

/// Whether a symbol is a thread parked doing nothing.
///
/// Idle workers dominate a self-time leaderboard otherwise: a 26-thread rayon
/// pool spends most of its wall clock asleep.
/// Idle samples are reported as a total plus a per-symbol breakdown rather than
/// dropped silently, so a thread genuinely blocked in `kevent` for the whole
/// run is still visible.
pub(crate) fn is_idle_symbol(symbol: &str) -> bool {
    const PARKED: &[&str] = &[
        "__psynch_cvwait",
        "_pthread_cond_wait",
        "__semwait_signal",
        "kevent",
        "mach_msg2_trap",
        "mach_msg_trap",
        "__workq_kernreturn",
        "start_wqthread",
        "_pthread_wqthread",
        "thread_start",
        "__select",
    ];

    PARKED.contains(&symbol)
        || symbol.contains("LockLatch>::wait")
        || symbol.contains("sleep::Sleep>::sleep")
}

/// Parse the call-graph section of a `sample(1)` output file.
///
/// Returns one [`Thread`] per `<count> Thread_XXX` header.
/// Lines outside the `Call graph:` section are skipped silently — the same
/// `awk` filter we'd run by hand, but built into the tool.
pub(crate) fn parse(text: &str) -> Vec<Thread> {
    let mut threads = Vec::new();
    let mut current: Option<Thread> = None;
    let mut in_graph = false;

    for line in text.lines() {
        if line.starts_with("Call graph:") {
            in_graph = true;
            continue;
        }
        if !in_graph {
            continue;
        }
        if line.starts_with("Total number in stack")
            || line.starts_with("Sort by top")
            || line.starts_with("Binary Images:")
        {
            // End of the section we care about.
            break;
        }

        match parse_line(line) {
            Some(LineKind::Header { text }) => {
                if let Some(thread) = current.take() {
                    threads.push(thread);
                }
                current = Some(Thread {
                    header: text,
                    frames: Vec::new(),
                });
            }
            Some(LineKind::Frame(frame)) => {
                if let Some(thread) = current.as_mut() {
                    thread.frames.push(frame);
                }
            }
            None => {}
        }
    }

    if let Some(thread) = current {
        threads.push(thread);
    }
    threads
}

/// One parsed line from the call-graph section.
enum LineKind {
    /// `<count> Thread_<id>[  description]` — introduces a new tree.
    Header { text: String },
    /// `[+!|: ]* <count> <symbol> (in <image>) + <offset> [<addr>]` — a frame
    /// in the current tree.
    Frame(Frame),
}

/// Parse a single line into either a thread header or a frame.
/// Returns `None` for blank lines, separators, or anything else that doesn't
/// match the expected shapes.
///
/// Both headers and frames share the prefix structure:
///
/// ```text
/// [whitespace + decoration glyphs] <integer count> <rest>
/// ```
///
/// We distinguish by inspecting `<rest>`: a leading `Thread_` marks a header,
/// anything else is a frame symbol.
fn parse_line(line: &str) -> Option<LineKind> {
    // Step 1: count leading whitespace + decoration glyphs as a proxy for
    // tree depth. Two chars per nesting step matches `sample(1)`'s output.
    let mut depth_chars = 0usize;
    let mut rest = line;
    while let Some(c) = rest.chars().next() {
        match c {
            ' ' | '+' | '!' | ':' | '|' => {
                depth_chars = depth_chars.saturating_add(1);
                rest = &rest[c.len_utf8()..];
            }
            _ => break,
        }
    }
    let depth = depth_chars / 2;

    // Step 2: the next whitespace-delimited token must be an integer count.
    let mut parts = rest.splitn(2, char::is_whitespace);
    let count_token = parts.next()?;
    let samples: u64 = count_token.parse().ok()?;
    let after_count = parts.next()?.trim_start();
    if after_count.is_empty() {
        return None;
    }

    // Step 3: dispatch on what follows the count.
    if after_count.starts_with("Thread_") {
        return Some(LineKind::Header {
            text: after_count.to_owned(),
        });
    }

    // Symbol: everything up to `  (in ` (the image marker). Trim trailing
    // whitespace before the marker so the resulting string doesn't carry
    // alignment padding.
    let end = after_count.find("  (in ").unwrap_or(after_count.len());
    let raw_symbol = after_count[..end].trim();
    if raw_symbol.is_empty() {
        return None;
    }

    // `{:#}` strips the trailing `[<hash>]` disambiguator from Rust v0
    // symbols, so `jp_cli[abcdef]::run` becomes `jp_cli::run`.
    let symbol = if raw_symbol.starts_with('_') {
        format!("{:#}", demangle(raw_symbol))
    } else {
        raw_symbol.to_owned()
    };

    Some(LineKind::Frame(Frame {
        depth,
        samples,
        symbol,
    }))
}

#[cfg(all(test, unix))]
#[path = "profile_sampling_parse_tests.rs"]
mod tests;
