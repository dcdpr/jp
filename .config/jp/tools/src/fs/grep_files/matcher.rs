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
}

impl Matcher for FancyMatcher {
    type Captures = NoCaptures;
    type Error = fancy_regex::Error;

    fn find_at(&self, haystack: &[u8], at: usize) -> Result<Option<Match>, Self::Error> {
        // `find_from_pos` starts the search at `at` while leaving the preceding
        // bytes visible, which is what lets lookbehind work across the offset.
        // Slicing the haystack instead would hide them.
        //
        // Bytes that are not valid UTF-8 simply never match, so one latin-1
        // line does not blind the search to the rest of the buffer.
        let found = self
            .regex
            .find_from_pos(haystack, at)?
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
