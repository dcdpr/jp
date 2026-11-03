//! The priority board file, [`PRIORITY_FILE`] in the RFD directory.
//!
//! It ranks open work into milestone groups above an unranked backlog:
//!
//! ```json
//! {
//!   "planned": [
//!     { "milestone": "Internal Release v0.1", "ids": ["044", "D50"] },
//!     { "milestone": null, "ids": ["053"] }
//!   ],
//!   "backlog": ["010", "D01"]
//! }
//! ```
//!
//! The site's board writes the same file when its rows are dragged, so
//! [`Priority::render`] lays it out the way the site does, and neither writer
//! leaves a diff for the other to undo.

use std::{fs, io};

use camino::{Utf8Path, Utf8PathBuf};
use serde::Serialize;
use serde_json::Value;

/// The board file, inside the RFD directory.
pub(crate) const PRIORITY_FILE: &str = ".priority.json";

/// One milestone group of the planned list: the RFDs ranked above that
/// milestone's marker, in order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct Group {
    /// `None` for the planned RFDs no milestone claims, which sit below the
    /// last marker.
    pub milestone: Option<String>,
    pub ids: Vec<String>,
}

/// The board file, normalized.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct Priority {
    /// The planned groups in board order, always ending with the unassigned
    /// (`milestone: None`) group.
    pub planned: Vec<Group>,

    /// The unranked backlog below the planned list.
    pub backlog: Vec<String>,
}

/// Where an id sits in the board file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Slot {
    /// At `index` in the planned group `group`.
    Planned { group: usize, index: usize },

    /// At `index` in the backlog.
    Backlog(usize),
}

impl Default for Priority {
    fn default() -> Self {
        Self::normalize(&Value::Null)
    }
}

impl Priority {
    /// Read a board file's contents into a fixed shape.
    ///
    /// The older shape, a flat `order` array in place of `planned`, reads as a
    /// single unassigned group.
    /// Missing or malformed fields read as empty, the way a missing file is an
    /// empty board.
    #[must_use]
    pub(crate) fn normalize(raw: &Value) -> Self {
        let mut planned: Vec<Group> = match raw.get("planned").and_then(Value::as_array) {
            Some(groups) => groups
                .iter()
                .map(|group| Group {
                    milestone: group.get("milestone").and_then(text),
                    ids: ids(group.get("ids")),
                })
                .collect(),
            None => vec![Group {
                milestone: None,
                ids: ids(raw.get("order")),
            }],
        };

        if planned.last().is_none_or(|group| group.milestone.is_some()) {
            planned.push(Group {
                milestone: None,
                ids: vec![],
            });
        }

        Self {
            planned,
            backlog: ids(raw.get("backlog")),
        }
    }

    /// The board file's contents, laid out the way the site's board writes
    /// them: two-space indentation and a trailing newline.
    #[must_use]
    pub(crate) fn render(&self) -> String {
        // Serializing plain strings and nulls can't fail.
        let mut json = serde_json::to_string_pretty(self).unwrap_or_default();
        json.push('\n');

        json
    }

    /// The planned ids, flattened in board order.
    ///
    /// Its length is the cutoff: a position below it is planned, one at or past
    /// it is in the backlog.
    #[must_use]
    pub(crate) fn order(&self) -> Vec<&str> {
        self.planned
            .iter()
            .flat_map(|group| group.ids.iter().map(String::as_str))
            .collect()
    }

    /// Every id on the board, planned then backlog, top first.
    pub(crate) fn placed(&self) -> impl Iterator<Item = &str> {
        self.planned
            .iter()
            .flat_map(|group| group.ids.iter())
            .chain(&self.backlog)
            .map(String::as_str)
    }

    /// Where `id` first appears.
    #[must_use]
    pub(crate) fn find(&self, id: &str) -> Option<Slot> {
        for (group, ids) in self.planned.iter().enumerate() {
            if let Some(index) = ids.ids.iter().position(|placed| placed == id) {
                return Some(Slot::Planned { group, index });
            }
        }

        self.backlog
            .iter()
            .position(|placed| placed == id)
            .map(Slot::Backlog)
    }

    /// Put `id` at `slot`, moving what was there down.
    pub(crate) fn insert(&mut self, slot: Slot, id: &str) {
        match slot {
            Slot::Planned { group, index } => self.planned[group].ids.insert(index, id.to_owned()),
            Slot::Backlog(index) => self.backlog.insert(index, id.to_owned()),
        }
    }

    /// Replace `old` with `new` wherever the board names it.
    /// `false` when it names `old` nowhere.
    pub(crate) fn rewrite(&mut self, old: &str, new: &str) -> bool {
        let mut changed = false;
        for id in self.ids_mut() {
            if id == old {
                new.clone_into(id);
                changed = true;
            }
        }

        changed
    }

    /// Take every id `ids` lists off the board.
    /// `false` when it names none of them.
    pub(crate) fn remove(&mut self, ids: &[&str]) -> bool {
        let before = self.placed().count();
        for group in &mut self.planned {
            group.ids.retain(|id| !ids.contains(&id.as_str()));
        }
        self.backlog.retain(|id| !ids.contains(&id.as_str()));

        self.placed().count() != before
    }

    fn ids_mut(&mut self) -> impl Iterator<Item = &mut String> {
        self.planned
            .iter_mut()
            .flat_map(|group| group.ids.iter_mut())
            .chain(self.backlog.iter_mut())
    }
}

/// An id list from the board file.
/// Numbers are read as their digits, since the file is hand-editable.
fn ids(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .map(|values| values.iter().filter_map(text).collect())
        .unwrap_or_default()
}

/// A string or a number from the board file, as text; `None` for anything else,
/// `null` included.
fn text(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        _ => None,
    }
}

/// Read the board file in the RFD directory `dir`.
///
/// `None` when there is no file, which is an empty board.
/// The error names the file and says what is wrong with it.
pub(crate) fn read(dir: &Utf8Path) -> Result<Option<Priority>, String> {
    let path = dir.join(PRIORITY_FILE);
    let source = match fs::read_to_string(&path) {
        Ok(source) => source,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("{path} could not be read ({error})")),
    };

    serde_json::from_str::<Value>(&source)
        .map(|raw| Some(Priority::normalize(&raw)))
        .map_err(|error| format!("{path} is not valid JSON ({error})"))
}

/// Read the board file before a command changes it.
///
/// Refuses a file that isn't JSON, so the command fails before it has written
/// anything else rather than after.
pub(crate) fn read_for_update(dir: &Utf8Path) -> Result<Option<Priority>, String> {
    read(dir).map_err(|error| {
        format!(
            "{error}, so the priority board can't be updated. Fix it and run the command again."
        )
    })
}

/// Write the board file into the RFD directory `dir`, returning its path.
pub(crate) fn write(dir: &Utf8Path, priority: &Priority) -> Result<Utf8PathBuf, String> {
    let path = dir.join(PRIORITY_FILE);
    fs::write(&path, priority.render()).map_err(|error| format!("{path}: {error}"))?;

    Ok(path)
}

#[cfg(test)]
#[path = "priority_tests.rs"]
mod tests;
