//! Convert assistant Markdown into Signal plain text, UTF-16 style ranges, and
//! message-sized chunks.
//!
//! Signal does not render Markdown directly; it accepts plain text accompanied
//! by `start:length:STYLE` ranges measured in UTF-16 code units.
//! [`styled`] converts headings and `**bold**` to `BOLD`, inline code and
//! fenced code blocks to `MONOSPACE`, `[label](url)` links to `label (url)`,
//! and `- ` / `* ` list items to `• `.
//! Unrecognized syntax is left as written.
//!
//! [`chunks`] splits long Markdown at paragraph boundaries before styling so
//! style ranges never cross message boundaries.
//! It budgets the UTF-8 size of each chunk's styled text, which is what Signal
//! measures: styling removes markup but turns each `- ` bullet into the larger
//! `• `.

/// Plain text and `start:length:STYLE` UTF-16 ranges for `signal-cli`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Styled {
    pub text: String,
    pub styles: Vec<String>,
}

/// Accumulator that tracks UTF-16 code unit offsets while building plain text.
#[derive(Default)]
struct Builder {
    text: String,
    units: usize,
    styles: Vec<String>,
}

impl Builder {
    fn push(&mut self, text: &str) {
        self.text.push_str(text);
        self.units += text.encode_utf16().count();
    }

    fn push_styled(&mut self, text: &str, style: &str) {
        let start = self.units;
        self.push(text);
        self.style(start, style);
    }

    fn style(&mut self, start: usize, style: &str) {
        let length = self.units - start;
        if length > 0 {
            self.styles.push(format!("{start}:{length}:{style}"));
        }
    }

    fn inline(&mut self, line: &str) {
        let mut rest = line;
        while !rest.is_empty() {
            if let Some(after) = rest.strip_prefix("**")
                && let Some(end) = after.find("**")
                && end > 0
            {
                self.push_styled(&after[..end], "BOLD");
                rest = &after[end + 2..];
            } else if let Some(after) = rest.strip_prefix('`')
                && let Some(end) = after.find('`')
                && end > 0
            {
                self.push_styled(&after[..end], "MONOSPACE");
                rest = &after[end + 1..];
            } else if let Some((text, url, after)) = link(rest) {
                self.push(&format!("{text} ({url})"));
                rest = after;
            } else {
                let next = rest.chars().next().map_or(1, char::len_utf8);
                self.push(&rest[..next]);
                rest = &rest[next..];
            }
        }
    }
}

/// Parse a `[label](url)` link at the start of `text`, returning `(label, url,
/// rest)`.
fn link(text: &str) -> Option<(&str, &str, &str)> {
    let after = text.strip_prefix('[')?;
    let close = after.find("](")?;
    let label = &after[..close];
    if label.contains('\n') {
        return None;
    }
    let target = &after[close + 2..];
    let end = target.find(')')?;

    Some((label, &target[..end], &target[end + 1..]))
}

/// Convert `markdown` into plain text and `signal-cli` style ranges.
pub(crate) fn styled(markdown: &str) -> Styled {
    let mut out = Builder::default();
    let mut fenced = false;
    let mut first = true;

    for line in markdown.trim().lines() {
        if line.trim_start().starts_with("```") {
            fenced = !fenced;
            continue;
        }
        if !first {
            out.push("\n");
        }
        first = false;

        if fenced {
            out.push_styled(line, "MONOSPACE");
            continue;
        }

        let trimmed = line.trim_start();
        let indent = &line[..line.len() - trimmed.len()];
        let hashes = trimmed.chars().take_while(|c| *c == '#').count();
        if (1..=6).contains(&hashes) && trimmed[hashes..].starts_with(' ') {
            let start = out.units;
            out.inline(trimmed[hashes..].trim());
            out.style(start, "BOLD");
        } else if let Some(item) = trimmed
            .strip_prefix("- ")
            .or_else(|| trimmed.strip_prefix("* "))
        {
            out.push(indent);
            out.push("• ");
            out.inline(item);
        } else {
            out.inline(line);
        }
    }

    Styled {
        text: out.text,
        styles: out.styles,
    }
}

/// UTF-8 size in bytes of `markdown` once [`styled`].
fn size(markdown: &str) -> usize {
    styled(markdown).text.len()
}

/// Split `markdown` into chunks whose styled text is at most `max` bytes,
/// preferring paragraph breaks outside code blocks, then line breaks, then
/// character boundaries.
pub(crate) fn chunks(markdown: &str, max: usize) -> Vec<String> {
    let mut chunks: Vec<String> = vec![];
    let mut current = String::new();

    for block in blocks(markdown.trim()) {
        for piece in fit(&block, max) {
            if !current.is_empty() && size(&format!("{current}\n\n{piece}")) > max {
                chunks.push(std::mem::take(&mut current));
            }
            if !current.is_empty() {
                current.push_str("\n\n");
            }
            current.push_str(&piece);
        }
    }
    if !current.is_empty() {
        chunks.push(current);
    }

    chunks
}

/// Split `markdown` into paragraph blocks while keeping fenced code blocks
/// intact.
fn blocks(markdown: &str) -> Vec<String> {
    let mut blocks = vec![];
    let mut current: Vec<&str> = vec![];
    let mut fenced = false;

    for line in markdown.lines() {
        if line.trim_start().starts_with("```") {
            fenced = !fenced;
        }
        if line.trim().is_empty() && !fenced {
            if !current.is_empty() {
                blocks.push(current.join("\n"));
                current.clear();
            }
            continue;
        }
        current.push(line);
    }
    if !current.is_empty() {
        blocks.push(current.join("\n"));
    }

    blocks
}

/// Split `block` into pieces whose styled text is at most `max` bytes, along
/// line or character boundaries.
fn fit(block: &str, max: usize) -> Vec<String> {
    if size(block) <= max {
        return vec![block.to_owned()];
    }

    let mut pieces: Vec<String> = vec![];
    let mut current = String::new();
    for line in block.lines() {
        let mut line = line.to_owned();
        while size(&line) > max {
            let cut = cut(&line, max);
            if !current.is_empty() {
                pieces.push(std::mem::take(&mut current));
            }
            pieces.push(line[..cut].to_owned());
            line = line[cut..].to_owned();
        }

        if !current.is_empty() && size(&format!("{current}\n{line}")) > max {
            pieces.push(std::mem::take(&mut current));
        }
        if !current.is_empty() {
            current.push('\n');
        }
        current.push_str(&line);
    }
    if !current.is_empty() {
        pieces.push(current);
    }

    pieces
}

/// The largest character boundary in `line` whose prefix styles to at most
/// `max` bytes, and at least one character so a split always makes progress.
fn cut(line: &str, max: usize) -> usize {
    let mut cut = line.floor_char_boundary(max.min(line.len()));
    while cut > 0 && size(&line[..cut]) > max {
        cut = line.floor_char_boundary(cut - 1);
    }

    if cut == 0 {
        line.chars().next().map_or(line.len(), char::len_utf8)
    } else {
        cut
    }
}

#[cfg(test)]
#[path = "text_tests.rs"]
mod tests;
