use std::str;

use fancy_regex::{Regex, RegexBuilder};
use grep_matcher::{Match, Matcher, NoCaptures};

/// A search matcher backed by `fancy-regex`.
///
/// Patterns get the same dialect as the rest of the toolset, so lookaround
/// (`(?=...)`, `(?<!...)`) and backreferences (`\1`) are available in addition
/// to the usual syntax.
pub(crate) struct FancyMatcher {
    regex: Regex,
}

impl FancyMatcher {
    pub(crate) fn new(pattern: &str) -> Result<Self, fancy_regex::Error> {
        // Line-oriented anchoring: `^` and `$` bind to line boundaries rather
        // than to the ends of the haystack, because the searcher hands over
        // whole buffers and callers write grep patterns, not whole-file ones.
        let regex = RegexBuilder::new(pattern)
            .multi_line(true)
            .dot_matches_new_line(false)
            .unicode_mode(true)
            .build()?;

        Ok(Self { regex })
    }

    /// Search each valid UTF-8 run of a haystack that is not valid UTF-8 as a
    /// whole, reporting offsets into the original bytes.
    ///
    /// Handing the malformed bytes to the engine directly loses matches: it
    /// advances over a stray byte by the width its UTF-8 lead byte claims, so a
    /// match starting in the bytes that width covers is never attempted.
    /// Lookaround cannot reach across a run boundary.
    fn find_in_valid_runs(
        &self,
        haystack: &[u8],
        at: usize,
    ) -> Result<Option<Match>, fancy_regex::Error> {
        let mut offset = 0;

        for chunk in haystack.utf8_chunks() {
            let run = chunk.valid();
            let end = offset + run.len();

            if at <= end
                && let Some(m) = self.regex.find_from_pos(run, at.saturating_sub(offset))?
            {
                return Ok(Some(Match::new(offset + m.start(), offset + m.end())));
            }

            offset = end + chunk.invalid().len();
        }

        Ok(None)
    }
}

impl Matcher for FancyMatcher {
    type Captures = NoCaptures;
    type Error = fancy_regex::Error;

    fn find_at(&self, haystack: &[u8], at: usize) -> Result<Option<Match>, Self::Error> {
        // The engine expects valid UTF-8, and a buffer holding a latin-1 line is
        // searched run by run instead.
        let Ok(text) = str::from_utf8(haystack) else {
            return self.find_in_valid_runs(haystack, at);
        };

        // `find_from_pos` starts the search at `at` while leaving the preceding
        // bytes visible, which is what lets lookbehind work across the offset.
        // Slicing the haystack instead would hide them.
        let found = self
            .regex
            .find_from_pos(text, at)?
            .map(|m| Match::new(m.start(), m.end()));

        Ok(found)
    }

    fn new_captures(&self) -> Result<Self::Captures, Self::Error> {
        Ok(NoCaptures::new())
    }
}

#[cfg(test)]
#[path = "matcher_tests.rs"]
mod tests;
