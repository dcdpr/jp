//! Checklist items in a ticket's description.
//!
//! A checklist item is a markdown task-list item: a list item whose text opens
//! with a checkbox, `[ ]` or `[x]`.
//!
//! ```markdown
//! - [x] Add the config types
//! - [ ] Enforce the cap
//! ```
//!
//! [`items`] reads them and [`progress`] counts them; neither is stored
//! anywhere, so a description edited by hand is never out of step with what the
//! tooling reports.
//! [`mark`] checks and unchecks items found by their text, changing the one
//! character inside each checkbox and nothing else.
//!
//! Only a ticket's description holds its checklist: the tooling only ever
//! appends comments, so an item in one could never be checked through it.
//! Items inside fenced code blocks don't count.
//!
//! The rule is per line: optional indentation, a list marker (`-`, `*`, `+`, or
//! up to nine digits and `.` or `)`), whitespace, the checkbox, whitespace, and
//! text.
//! It doesn't follow markdown's block structure any further, so an item quoted
//! in a blockquote doesn't count, and one under an indented code block does.
//! The site's reader in `jp-vite` (`loaders/ticket-shared.mjs`) applies the
//! same rule, and `checklist_cases.json` holds the cases both are tested
//! against.

use std::fmt;

use serde::Serialize;

use crate::parse::Fences;

/// One checklist item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    /// The 0-based line of the description it is on, counting `\n`s.
    pub line: usize,

    pub checked: bool,

    /// What follows the checkbox on its line, trimmed, inline markdown and all.
    pub text: String,
}

/// How many of a description's checklist items are checked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Progress {
    pub checked: usize,
    pub count: usize,
}

impl Progress {
    /// How many items are still unchecked.
    #[must_use]
    pub fn open(&self) -> usize {
        self.count - self.checked
    }
}

impl fmt::Display for Progress {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} of {} checked", self.checked, self.count)
    }
}

/// Every checklist item in `description`, in order.
#[must_use]
pub fn items(description: &str) -> Vec<Item> {
    let mut fences = Fences::default();

    description
        .split('\n')
        .enumerate()
        .filter(|(_, line)| !fences.consume(line))
        .filter_map(|(index, line)| {
            let checkbox = checkbox(line)?;
            Some(Item {
                line: index,
                checked: checkbox.checked,
                text: line[checkbox.text..].trim().to_owned(),
            })
        })
        .collect()
}

/// How many of `description`'s checklist items are checked, or `None` when it
/// holds none.
#[must_use]
pub fn progress(description: &str) -> Option<Progress> {
    progress_of(&items(description))
}

fn progress_of(items: &[Item]) -> Option<Progress> {
    if items.is_empty() {
        return None;
    }

    Some(Progress {
        checked: items.iter().filter(|item| item.checked).count(),
        count: items.len(),
    })
}

/// What one item named in a [`mark`] call ended up as.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    /// The item's text.
    pub text: String,

    /// Whether it is checked now.
    pub checked: bool,

    /// Whether the call changed it, rather than finding it so already.
    pub changed: bool,
}

/// A description after [`mark`] checked and unchecked items in it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Marked {
    pub description: String,

    /// Each item named, once, in the order first named.
    pub changes: Vec<Change>,

    /// The checklist's progress after the change.
    pub progress: Progress,
}

/// Why [`mark`] changed nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refused {
    problems: Vec<Problem>,

    /// Every item in the description, listed when a query matched none.
    items: Vec<Item>,
}

/// One thing wrong with the items a [`mark`] call named.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Problem {
    /// The description holds no checklist items.
    NoItems,

    /// A query that was blank.
    EmptyQuery,

    /// No item's text contains the query.
    NoMatch(String),

    /// More than one item's text contains the query, and no single one is all
    /// of it.
    Ambiguous {
        query: String,
        candidates: Vec<String>,
    },

    /// One item named to be both checked and unchecked.
    Conflict(String),
}

impl fmt::Display for Refused {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let lines: Vec<String> = self.problems.iter().map(ToString::to_string).collect();
        f.write_str(&lines.join("\n"))?;

        if self
            .problems
            .iter()
            .any(|problem| matches!(problem, Problem::NoMatch(_)))
        {
            f.write_str("\nThe checklist items are:")?;
            for item in &self.items {
                let mark = if item.checked { 'x' } else { ' ' };
                write!(f, "\n- [{mark}] {}", item.text)?;
            }
        }

        Ok(())
    }
}

impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoItems => f.write_str(
                "The description holds no checklist items (`- [ ] ...`) to check or uncheck.",
            ),
            Self::EmptyQuery => f.write_str("A checklist item to find can't be empty."),
            Self::NoMatch(query) => write!(f, "No checklist item contains `{query}`."),
            Self::Ambiguous { query, candidates } => {
                let quoted: Vec<String> =
                    candidates.iter().map(|text| format!("{text:?}")).collect();
                write!(
                    f,
                    "`{query}` matches more than one checklist item: {}. Name more of the one you \
                     mean.",
                    quoted.join(", ")
                )
            }
            Self::Conflict(text) => {
                write!(f, "`{text}` is named to be both checked and unchecked.")
            }
        }
    }
}

impl std::error::Error for Refused {}

/// Check the items `check` names and uncheck the ones `uncheck` names.
///
/// An item is named by its text, ignoring case: by all of it, which wins when
/// another item's text contains it too, or by any part that only one item
/// contains.
/// An item already in the state asked for is reported unchanged rather than
/// refused, so the same call can be made twice.
///
/// Every name is resolved before anything changes, and every problem is
/// reported together: a name matching no item, or several, and an item named
/// both ways.
/// Only the character inside each checkbox changes; a checked `[X]` asked to be
/// checked keeps its capital.
///
/// # Errors
///
/// Returns [`Refused`] naming what is wrong with the names, and listing the
/// items when a name matched none of them.
pub fn mark(description: &str, check: &[String], uncheck: &[String]) -> Result<Marked, Refused> {
    let items = items(description);
    if items.is_empty() {
        return Err(Refused {
            problems: vec![Problem::NoItems],
            items,
        });
    }

    let mut problems = vec![];
    // Item index and the state asked for, each item once.
    let mut wanted: Vec<(usize, bool)> = vec![];
    let named = check
        .iter()
        .map(|query| (query, true))
        .chain(uncheck.iter().map(|query| (query, false)));

    for (query, checked) in named {
        let index = match find(&items, query) {
            Ok(index) => index,
            Err(problem) => {
                problems.push(problem);
                continue;
            }
        };

        match wanted.iter().find(|(known, _)| *known == index) {
            None => wanted.push((index, checked)),
            Some((_, already)) if *already == checked => {}
            Some(_) => {
                let conflict = Problem::Conflict(items[index].text.clone());
                if !problems.contains(&conflict) {
                    problems.push(conflict);
                }
            }
        }
    }

    if !problems.is_empty() {
        return Err(Refused { problems, items });
    }

    let mut lines: Vec<String> = description.split('\n').map(ToOwned::to_owned).collect();
    let mut after = items.clone();
    let mut changes = vec![];
    for (index, checked) in wanted {
        let item = &items[index];
        let changed = item.checked != checked;
        if changed {
            let line = &mut lines[item.line];
            if let Some(checkbox) = checkbox(line) {
                let mark = if checked { "x" } else { " " };
                line.replace_range(checkbox.mark..=checkbox.mark, mark);
            }
            after[index].checked = checked;
        }

        changes.push(Change {
            text: item.text.clone(),
            checked,
            changed,
        });
    }

    Ok(Marked {
        description: lines.join("\n"),
        changes,
        progress: Progress {
            checked: after.iter().filter(|item| item.checked).count(),
            count: after.len(),
        },
    })
}

/// The one item `query` names: the one whose whole text it is, or else the one
/// whose text contains it, ignoring case either way.
fn find(items: &[Item], query: &str) -> Result<usize, Problem> {
    let query = query.trim();
    if query.is_empty() {
        return Err(Problem::EmptyQuery);
    }

    let folded = query.to_lowercase();
    let whole: Vec<usize> = (0..items.len())
        .filter(|&index| items[index].text.to_lowercase() == folded)
        .collect();
    let candidates = if whole.is_empty() {
        (0..items.len())
            .filter(|&index| items[index].text.to_lowercase().contains(&folded))
            .collect()
    } else {
        whole
    };

    match candidates.as_slice() {
        [] => Err(Problem::NoMatch(query.to_owned())),
        [index] => Ok(*index),
        several => Err(Problem::Ambiguous {
            query: query.to_owned(),
            candidates: several
                .iter()
                .map(|&index| items[index].text.clone())
                .collect(),
        }),
    }
}

/// Where a checklist item's parts sit in its line, as byte offsets.
struct Checkbox {
    /// The character inside the brackets.
    mark: usize,

    checked: bool,

    /// Where the text after the checkbox starts.
    text: usize,
}

/// Read `line` as a checklist item.
fn checkbox(line: &str) -> Option<Checkbox> {
    let bytes = line.as_bytes();
    let blank = |byte: u8| byte == b' ' || byte == b'\t';

    let mut at = bytes.iter().take_while(|byte| blank(**byte)).count();

    // The list marker: a bullet, or up to nine digits and a delimiter.
    match bytes.get(at)? {
        b'-' | b'*' | b'+' => at += 1,
        byte if byte.is_ascii_digit() => {
            let digits = bytes[at..]
                .iter()
                .take_while(|byte| byte.is_ascii_digit())
                .count();
            if digits > 9 || !matches!(bytes.get(at + digits)?, b'.' | b')') {
                return None;
            }
            at += digits + 1;
        }
        _ => return None,
    }

    let gap = bytes[at..].iter().take_while(|byte| blank(**byte)).count();
    if gap == 0 {
        return None;
    }
    at += gap;

    if bytes.get(at) != Some(&b'[') || bytes.get(at + 2) != Some(&b']') {
        return None;
    }
    let checked = match bytes[at + 1] {
        b' ' => false,
        b'x' | b'X' => true,
        _ => return None,
    };
    let mark = at + 1;
    at += 3;

    if !bytes.get(at).copied().is_some_and(blank) || line[at..].trim().is_empty() {
        return None;
    }

    Some(Checkbox {
        mark,
        checked,
        text: at,
    })
}

#[cfg(test)]
#[path = "checklist_tests.rs"]
mod tests;
