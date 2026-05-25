//! Sentence segmentation with abbreviation-aware merging.
//!
//! Adapted from snapper-fmt (<https://github.com/TurtleTech-ehf/snapper>),
//! MIT-licensed, Copyright (c) 2026 Rohit Goswami.
//!
//! Reduced to the English-only subset comfort actually needs and inlined to
//! avoid the upstream dependency.
//! Logic is otherwise unchanged: protect inline tokens (URLs, code spans,
//! links) with placeholders, run UAX \#29 sentence segmentation, then merge
//! false splits caused by abbreviations and quoted punctuation.

use std::{ops::Range, sync::LazyLock};

use regex::Regex;
use unicode_segmentation::UnicodeSegmentation;

/// English abbreviations whose trailing period must not be treated as a
/// sentence boundary.
/// Kept short and code-comment-focused.
static EN_ABBREVIATIONS: &[&str] = &[
    // Titles
    "Mr", "Mrs", "Ms", "Dr", "Prof", "Sr", "Jr", "St", "Rev", "Gen", "Gov", "Sgt", "Cpl", "Pvt",
    "Capt", "Lt", "Col", "Maj", "Cmdr", "Adm", // Academic / scientific
    "Fig", "Figs", "Eq", "Eqs", "Ref", "Refs", "Tab", "Sec", "Ch", "Vol", "No", "Nos", "Ed", "Eds",
    "Trans", "Dept", "Thm", "Lem", "Prop", "Def", "Cor", "Rem", "Ex", // Latin
    "al", "approx", "ca", "cf", "etc", "et", "ibid", "viz", // Common
    "vs", "misc", "est", "govt", "dept", "univ", "inc", "corp", "ltd", "Ave", "Blvd", "Rd", "Jan",
    "Feb", "Mar", "Apr", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec", "Mon", "Tue", "Wed",
    "Thu", "Fri", "Sat", "Sun", "pp", "pg", "pt", "pts", // Single letters (initials)
    "A", "B", "C", "D", "E", "F", "G", "H", "I", "J", "K", "L", "M", "N", "O", "P", "Q", "R", "S",
    "T", "U", "V", "W", "X", "Y", "Z",
];

/// Multi-word abbreviations where the period falls inside, e.g. `e.g.`, `i.e.`,
/// `a.m.`, `p.m.`, `v.s.`.
static EN_MULTI_ABBREVS: &[&str] = &["e.g", "i.e", "a.m", "p.m", "v.s"];

/// Inline tokens that must not be broken across sentences.
/// Replaced with placeholders before segmentation, restored after.
static INLINE_TOKEN_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        &[
            r"\[\[[^\]]*\]\]",                  // Org links: [[url]]
            r"\[\[[^\]]*\]\[[^\]]*\]\]",        // Org links with description
            r"\[[^\]]+\]\([^)]+\)",             // Markdown inline links
            r"!\[[^\]]*\]\([^)]+\)",            // Markdown images
            r"\$[^$]+\$",                       // Inline math
            r"\\([a-zA-Z]+)\{[^}]*\}",          // LaTeX commands
            r"~[^~]+~",                         // Org inline code
            r"=[^=]+=",                         // Org verbatim
            r"`[^`]+`",                         // Markdown inline code
            r"\*\*[^*]+\*\*",                   // Markdown bold: **text**
            r"~~[^~]+~~",                       // Markdown strikethrough: ~~text~~
            r#"https?://\S+[^.\s!?,;:)\]'""]"#, // URLs (don't swallow trailing punct)
            r"file:\S+",                        // file:// links
        ]
        .join("|"),
    )
    .expect("valid inline-token regex")
});

/// Punctuation followed by closing quote/paren at the end of a segment.
/// Used to detect false splits like `He said "wow!"` + `and left.`.
static QUOTED_PUNCT_END_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"[.!?]["')\]]+\s*$"#).expect("valid quoted-punct regex"));

/// Compiled regex matching a single-token abbreviation immediately before a
/// trailing period.
/// Anchored to end of segment.
static ABBREV_RE: LazyLock<Regex> = LazyLock::new(|| {
    let alts = EN_ABBREVIATIONS.join("|");
    let pattern = format!(r#"(?:^|[\s"'`(\[])(?:{alts})$"#);
    Regex::new(&pattern).expect("valid abbreviation regex")
});

/// Compiled regex matching a multi-word abbreviation immediately before a
/// trailing period.
static MULTI_ABBREV_RE: LazyLock<Regex> = LazyLock::new(|| {
    let alts: Vec<String> = EN_MULTI_ABBREVS.iter().map(|a| regex::escape(a)).collect();
    let pattern = format!(r"(?:^|\s)(?:{})$", alts.join("|"));
    Regex::new(&pattern).expect("valid multi-abbreviation regex")
});

/// Split a prose paragraph into individual sentences, respecting common
/// abbreviations and inline-token boundaries.
///
/// `atomic_ranges` are byte ranges in `text` that must be treated as
/// indivisible by sentence segmentation: typically markdown inline spans
/// (`Emph`, `Strong`, `Strikethrough`, `Code`, `Link`, etc.) whose byte extents
/// come from the AST walker in [`format`].
/// Pass `&[]` for the standalone path; in that case only `INLINE_TOKEN_RE`
/// regex protection applies.
///
/// Ranges that overlap with earlier ones (or with regex matches in the same
/// position) are dropped; the first match wins.
///
/// [`format`]: crate::format
#[must_use]
pub fn split_sentences(text: &str, atomic_ranges: &[Range<usize>]) -> Vec<String> {
    // Trim and adjust caller-provided ranges to the trimmed slice. Atomic
    // ranges typically arrive aligned to `text` exactly (the AST walker
    // computes them from sourcepos relative to the paragraph's start),
    // but the trim is defensive.
    let leading = text.len() - text.trim_start().len();
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }

    // Gather every protected span: caller-provided atomic ranges first,
    // then regex matches for the patterns we can't reliably get from the
    // AST (bare URLs, file:// links, org-mode tokens, etc.). Dropped if
    // out-of-bounds or not at char boundaries.
    let mut protected: Vec<Range<usize>> = Vec::new();
    for r in atomic_ranges {
        let Some(start) = r.start.checked_sub(leading) else {
            continue;
        };
        let Some(end) = r.end.checked_sub(leading) else {
            continue;
        };
        if start < end
            && end <= trimmed.len()
            && trimmed.is_char_boundary(start)
            && trimmed.is_char_boundary(end)
        {
            protected.push(start..end);
        }
    }
    for m in INLINE_TOKEN_RE.find_iter(trimmed) {
        protected.push(m.start()..m.end());
    }
    // Sort by start; drop ranges that overlap with an earlier one
    // (earlier always wins).
    protected.sort_by_key(|r| r.start);
    let mut non_overlapping: Vec<Range<usize>> = Vec::new();
    let mut max_end = 0;
    for r in protected {
        if r.start >= max_end {
            max_end = r.end;
            non_overlapping.push(r);
        }
    }

    // Substitute placeholders in a single forward pass. Placeholders use
    // NUL to avoid colliding with any normal text content.
    //
    // The atomic content goes through `fold_line_breaks` first: a span
    // whose source crosses a line boundary (e.g. an italic that wraps
    // across two markdown lines with a continuation indent) would
    // otherwise leak the embedded `\n  ` into the placeholder. textwrap
    // treats `\n` as a forced break, and the downstream container
    // prefix step would then add its own continuation indent on top of
    // the preserved source indent — producing visibly over-indented
    // output. Folding line breaks to a single space matches CommonMark's
    // rendering rule for inline spans.
    let mut placeholders: Vec<String> = Vec::new();
    let mut substituted = String::with_capacity(trimmed.len());
    let mut cursor = 0;
    for r in &non_overlapping {
        substituted.push_str(&trimmed[cursor..r.start]);
        let original = fold_line_breaks(&trimmed[r.clone()]);
        let idx = placeholders.len();
        substituted.push_str(&format!("\x00PH{idx}\x00"));
        placeholders.push(original);
        cursor = r.end;
    }
    substituted.push_str(&trimmed[cursor..]);

    // Collapse runs of whitespace (newlines, tabs, multiple spaces) into a
    // single space. Markdown renders soft line breaks as spaces; if we skip
    // this step, embedded `\n` from the source comes through into each
    // sentence and breaks textwrap's notion of where lines start. Safe to
    // run after placeholder substitution because placeholders
    // (`\x00PH<n>\x00`) contain no whitespace.
    let normalized = collapse_whitespace(&substituted);

    let raw_segments: Vec<&str> = normalized.unicode_sentences().collect();
    if raw_segments.is_empty() {
        return vec![trimmed.to_owned()];
    }

    let merged = merge_abbreviation_splits(&raw_segments);
    let merged = merge_quoted_punct_splits(merged);

    merged
        .into_iter()
        .map(|s| restore_placeholders(s.trim(), &placeholders))
        .filter(|s| !s.is_empty())
        .collect()
}

/// Replace any newline (CR or LF) followed by horizontal whitespace with a
/// single space.
/// Multi-space runs that don't include a newline are left alone (matching
/// CommonMark's preservation of literal spaces in inline code, and avoiding
/// surprising changes elsewhere).
///
/// Used to fold the contents of atomic spans (emphasis, inline code, links)
/// that happen to cross a source-line boundary before they're stored as
/// placeholders; without this, textwrap would later treat the embedded ` \n  `
/// as a forced break and the container-prefix step would double up the
/// continuation indent.
fn fold_line_breaks(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\n' || c == '\r' {
            out.push(' ');
            while chars.peek().is_some_and(|next| matches!(*next, ' ' | '\t')) {
                chars.next();
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Collapse every run of Unicode whitespace into a single ASCII space.
/// Used to normalise markdown paragraph content (soft line breaks, indent on
/// continuation lines, accidental double spaces) before sentence segmentation.
fn collapse_whitespace(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_ws = false;
    for c in s.chars() {
        if c.is_whitespace() {
            if !in_ws {
                out.push(' ');
                in_ws = true;
            }
        } else {
            out.push(c);
            in_ws = false;
        }
    }
    out.trim().to_owned()
}

fn restore_placeholders(s: &str, placeholders: &[String]) -> String {
    let mut restored = s.to_owned();
    for (i, original) in placeholders.iter().enumerate() {
        let ph = format!("\x00PH{i}\x00");
        restored = restored.replace(&ph, original);
    }
    restored
}

/// Re-join consecutive segments when the earlier one ends in a known
/// abbreviation; UAX \#29 doesn't know about these and false-splits.
fn merge_abbreviation_splits(segments: &[&str]) -> Vec<String> {
    let mut result: Vec<String> = Vec::with_capacity(segments.len());
    for &segment in segments {
        let merge = result
            .last()
            .is_some_and(|prev| is_abbreviation_ending(prev));
        if merge {
            result.last_mut().unwrap().push_str(segment);
        } else {
            result.push(segment.to_owned());
        }
    }
    result
}

/// Re-join when a segment ends with sentence punctuation inside closing
/// quotes/parens AND the next segment starts with a lowercase letter, meaning
/// the apparent break is actually mid-sentence.
/// E.g.
/// `He said "wow!" and left.` is one sentence, not two.
fn merge_quoted_punct_splits(segments: Vec<String>) -> Vec<String> {
    let mut result: Vec<String> = Vec::with_capacity(segments.len());
    for segment in segments {
        let merge = result.last().is_some_and(|prev| {
            QUOTED_PUNCT_END_RE.is_match(prev.trim_end())
                && segment
                    .trim_start()
                    .chars()
                    .next()
                    .is_some_and(char::is_lowercase)
        });
        if merge {
            result.last_mut().unwrap().push_str(&segment);
        } else {
            result.push(segment);
        }
    }
    result
}

fn is_abbreviation_ending(s: &str) -> bool {
    let trimmed = s.trim_end();
    if !trimmed.ends_with('.') {
        return false;
    }
    let before_dot = &trimmed[..trimmed.len() - 1];
    ABBREV_RE.is_match(before_dot) || MULTI_ABBREV_RE.is_match(before_dot)
}

#[cfg(test)]
#[path = "sentence_tests.rs"]
mod tests;
