//! How tickets name RFDs: two label keys, both settings.
//!
//! A ticket labelled `tracking-rfd=116` is RFD 116's tracking ticket: it stands
//! for the implementation as a whole, and the RFD is in development while it is
//! In Progress.
//! A ticket labelled `relevant-rfd=091` is part of RFD 091's work, and can name
//! several RFDs that way.
//!
//! The keys are `plugins.command.rfd.options.tracking.key` and `relevant.key`,
//! so a board names them its own way, and the ticket format knows nothing about
//! RFDs.
//! A value is read leniently, so `116`, `RFD 116`, and `d5` each name an RFD;
//! what `jp rfd` writes is the canonical form, `116` or `D05`.

use std::{collections::BTreeMap, fs};

use camino::Utf8Path;
use serde::Serialize;
use ticket::{LABEL_KEY, Labels, Status, parse, render, store};

use crate::id::RfdId;

/// The key a tracking ticket names its RFD under when none is configured.
pub(crate) const DEFAULT_TRACKING_KEY: &str = "tracking-rfd";

/// The key a ticket names an RFD its work belongs to under when none is
/// configured.
pub(crate) const DEFAULT_RELEVANT_KEY: &str = "relevant-rfd";

/// The label keys tickets name RFDs under, and the labels a tracking ticket is
/// filed with besides its link.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Links {
    /// The key whose value names the RFD a ticket tracks.
    pub tracking: String,

    /// The key whose value names an RFD a ticket's work belongs to.
    pub relevant: String,

    /// The labels a tracking ticket is filed with besides its link, such as
    /// `type=tracking`.
    pub filed_with: Vec<String>,
}

impl Default for Links {
    fn default() -> Self {
        Self {
            tracking: DEFAULT_TRACKING_KEY.to_owned(),
            relevant: DEFAULT_RELEVANT_KEY.to_owned(),
            filed_with: vec![],
        }
    }
}

/// A ticket that names an RFD.
///
/// Serialized as one of an RFD's `tickets` in `jp -F json rfd list` and `show`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct LinkedTicket {
    /// The ticket's id, `T-02wt0k1`.
    pub id: String,

    pub title: String,

    pub status: Status,

    /// Whether the ticket tracks the RFD, rather than naming it as one its work
    /// belongs to.
    pub tracking: bool,
}

impl Links {
    /// The RFDs `labels` names as the ones its ticket tracks.
    pub(crate) fn tracked(&self, labels: &Labels) -> Vec<RfdId> {
        named(labels, &self.tracking)
    }

    /// The RFDs `labels` names as ones its ticket's work belongs to.
    pub(crate) fn relevant(&self, labels: &Labels) -> Vec<RfdId> {
        named(labels, &self.relevant)
    }

    /// The tickets among `entries` that name an RFD, by the RFD's canonical id:
    /// the ones tracking it first, then the rest, each in id order.
    ///
    /// A ticket naming one RFD both ways counts once, as tracking it.
    /// Tickets that didn't parse are passed over.
    pub(crate) fn by_rfd(&self, entries: &[store::Entry]) -> BTreeMap<String, Vec<LinkedTicket>> {
        let mut by_rfd: BTreeMap<String, Vec<LinkedTicket>> = BTreeMap::new();

        for entry in entries {
            let Ok(ticket) = &entry.ticket else {
                continue;
            };

            let mut named: Vec<(RfdId, bool)> = self
                .tracked(&ticket.metadata.labels)
                .into_iter()
                .map(|id| (id, true))
                .collect();
            for id in self.relevant(&ticket.metadata.labels) {
                if !named.iter().any(|(known, _)| *known == id) {
                    named.push((id, false));
                }
            }

            for (id, tracking) in named {
                by_rfd
                    .entry(id.to_string())
                    .or_default()
                    .push(LinkedTicket {
                        id: entry.id.to_string(),
                        title: ticket.title.clone(),
                        status: ticket.metadata.status,
                        tracking,
                    });
            }
        }

        for tickets in by_rfd.values_mut() {
            tickets.sort_by(|a, b| b.tracking.cmp(&a.tracking).then_with(|| a.id.cmp(&b.id)));
        }

        by_rfd
    }

    /// Every label a tracking ticket for `id` is filed with, its link first.
    pub(crate) fn tracking_labels(&self, id: RfdId) -> Vec<String> {
        let mut labels = vec![format!("{}={id}", self.tracking)];
        labels.extend(self.filed_with.iter().cloned());

        labels
    }

    /// The labels a tracking ticket for `id` is filed with, the way a sentence
    /// names them: `` `tracking-rfd=116` and `type=tracking` ``.
    pub(crate) fn describe_tracking(&self, id: RfdId) -> String {
        let labels: Vec<String> = self
            .tracking_labels(id)
            .iter()
            .map(|label| format!("`{label}`"))
            .collect();

        match labels.split_last() {
            Some((last, rest)) if !rest.is_empty() => format!("{} and {last}", rest.join(", ")),
            _ => labels.concat(),
        }
    }

    /// Point the labels of the ticket file at `path` that name `old` at `new`
    /// instead, returning whether the file changed.
    ///
    /// A file without a metadata block, such as the ticket index, is left
    /// alone.
    pub(crate) fn relink(&self, path: &Utf8Path, old: RfdId, new: RfdId) -> Result<bool, String> {
        let source = fs::read_to_string(path).map_err(|error| format!("{path}: {error}"))?;

        let mut relinked = Labels::default();
        let mut changed = false;
        for (key, values) in &parse::labels(&source) {
            let linking = key.eq_ignore_ascii_case(&self.tracking)
                || key.eq_ignore_ascii_case(&self.relevant);
            for value in values {
                let value = match value.parse::<RfdId>() {
                    Ok(id) if linking && id == old => {
                        changed = true;
                        new.to_string()
                    }
                    _ => value.clone(),
                };
                relinked.insert(key.clone(), value);
            }
        }

        if !changed {
            return Ok(false);
        }
        let Some(updated) =
            render::set_repeated_metadata(&source, LABEL_KEY, &relinked.to_tokens())
        else {
            return Ok(false);
        };

        fs::write(path, updated).map_err(|error| format!("{path}: {error}"))?;
        Ok(true)
    }
}

/// The RFDs `labels` names under `key`, each once, matching the key without
/// case and passing over a value that names no RFD.
fn named(labels: &Labels, key: &str) -> Vec<RfdId> {
    let mut ids = vec![];
    for (known, values) in labels {
        if !known.eq_ignore_ascii_case(key) {
            continue;
        }
        for value in values {
            if let Ok(id) = value.parse::<RfdId>()
                && !ids.contains(&id)
            {
                ids.push(id);
            }
        }
    }

    ids
}

#[cfg(test)]
#[path = "links_tests.rs"]
mod tests;
