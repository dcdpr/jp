//! Reading a target's `name`, and finding it on a line or among the symbols the
//! server returned.

use super::{protocol::SymbolInformation, rust_analyzer::PATH_SEPARATOR};

/// A symbol name as the assistant writes it: `iter`, `Stream::iter`, or
/// `stream::Stream`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Name {
    /// The leading segments, outermost first.
    pub qualifier: Vec<String>,

    /// The last segment.
    pub ident: String,
}

impl Name {
    /// Parse a possibly qualified name.
    ///
    /// A trailing `()` is dropped, so `Stream::iter()` names the method.
    pub fn parse(raw: &str) -> Result<Self, String> {
        let trimmed = raw.trim().trim_end_matches("()");
        let mut segments = trimmed
            .split(PATH_SEPARATOR)
            .map(str::trim)
            .filter(|segment| !segment.is_empty())
            .map(str::to_owned)
            .collect::<Vec<_>>();

        let ident = segments
            .pop()
            .ok_or_else(|| "`name` is empty.".to_owned())?;

        let bare = ident.strip_prefix("r#").unwrap_or(&ident);
        if bare.is_empty() || !bare.chars().all(is_ident_char) {
            return Err(format!(
                "`{raw}` does not end in an identifier. Give a name such as `iter` or \
                 `Stream::iter`."
            ));
        }

        Ok(Self {
            qualifier: segments,
            ident,
        })
    }

    /// The last segment without a raw identifier's `r#`, as the server's symbol
    /// search reports names.
    #[must_use]
    pub fn bare_ident(&self) -> &str {
        self.ident.strip_prefix("r#").unwrap_or(&self.ident)
    }

    /// The name as written, qualifier included.
    #[must_use]
    pub fn query(&self) -> String {
        self.qualifier
            .iter()
            .chain(std::iter::once(&self.ident))
            .cloned()
            .collect::<Vec<_>>()
            .join(PATH_SEPARATOR)
    }

    /// The name without its last segment, if it has a qualifier.
    #[must_use]
    pub fn parent(&self) -> Option<Self> {
        let (ident, qualifier) = self.qualifier.split_last()?;

        Some(Self {
            qualifier: qualifier.to_vec(),
            ident: ident.clone(),
        })
    }
}

fn is_ident_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// The byte offsets where `ident` appears in `line` as a whole word.
#[must_use]
pub fn occurrences(line: &str, ident: &str) -> Vec<usize> {
    line.match_indices(ident)
        .filter(|(at, _)| {
            let before = line[..*at].chars().next_back();
            let after = line[at + ident.len()..].chars().next();

            !before.is_some_and(is_ident_char) && !after.is_some_and(is_ident_char)
        })
        .map(|(at, _)| at)
        .collect()
}

/// The byte offset of the occurrence of `ident` on `line` that the target
/// names.
///
/// `occurrence` counts from 1, and is only needed when `ident` appears more
/// than once.
pub fn pick(line: &str, ident: &str, occurrence: Option<usize>) -> Result<usize, String> {
    let found = occurrences(line, ident);

    match (found.as_slice(), occurrence) {
        ([], _) => Err(format!("`{ident}` does not appear on that line.")),
        ([only], None) => Ok(*only),
        (all, Some(n)) => all.get(n.wrapping_sub(1)).copied().ok_or_else(|| {
            format!(
                "`{ident}` appears {} times on that line, so `occurrence` must be 1 to {}.",
                all.len(),
                all.len()
            )
        }),
        (all, None) => Err(format!(
            "`{ident}` appears {} times on that line. Pass `occurrence` to pick one, counting \
             from 1.",
            all.len()
        )),
    }
}

/// Whether `qualifier` fits where a symbol sits.
///
/// The qualifier's segments must appear, in order, among the components of the
/// symbol's workspace-relative `path` (without the `.rs`) followed by the names
/// in its `container`, the type or trait it is defined on.
/// `Stream::iter` fits a method on `Stream<T>`, and
/// `jp_conversation::ConversationStream` fits a struct in
/// `crates/jp_conversation/src/stream.rs`.
#[must_use]
pub fn qualifier_fits(qualifier: &[String], path: &str, container: Option<&str>) -> bool {
    let path = path.strip_suffix(".rs").unwrap_or(path);
    let mut scope = path
        .split('/')
        .chain(
            container
                .unwrap_or_default()
                .split(|c: char| !is_ident_char(c)),
        )
        .filter(|segment| !segment.is_empty());

    qualifier
        .iter()
        .all(|wanted| scope.any(|segment| segment == wanted))
}

/// The symbols named exactly `ident`, once per location.
///
/// The server's search is fuzzy and case-insensitive, so `iter` also returns
/// `iter_turns` and `Iter`.
#[must_use]
pub fn exact(symbols: Vec<SymbolInformation>, ident: &str) -> Vec<SymbolInformation> {
    let mut found: Vec<SymbolInformation> = vec![];
    for symbol in symbols {
        if symbol.name == ident && !found.iter().any(|seen| seen.location == symbol.location) {
            found.push(symbol);
        }
    }

    found
}

#[cfg(test)]
#[path = "target_tests.rs"]
mod tests;
