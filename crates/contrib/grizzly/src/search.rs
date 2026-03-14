use std::{
    collections::{BTreeSet, HashSet},
    rc::Rc,
};

use rusqlite::{Connection, types::Value};

use crate::{Result, fts};

/// Controls which search backend to use.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SearchMode {
    /// Try FTS5 first, fall back to LIKE on error or empty results.
    #[default]
    Auto,

    /// Force FTS5 (no LIKE fallback).
    Fts,

    /// Force LIKE (original behavior).
    Like,
}

/// Parameters for a note search.
pub struct SearchParams {
    /// Search queries (matched with LIKE against title and content).
    pub queries: Vec<String>,

    /// Only search notes with ALL of these tags.
    pub tags: Vec<String>,

    /// Only search notes with these IDs.
    pub ids: Vec<String>,

    /// Number of context lines around each match.
    pub context: usize,

    /// Maximum number of notes to return (default: 50).
    pub limit: usize,

    /// Search backend to use.
    pub mode: SearchMode,
}

impl Default for SearchParams {
    fn default() -> Self {
        Self {
            queries: vec![],
            tags: vec![],
            ids: vec![],
            context: 3,
            limit: 50,
            mode: SearchMode::default(),
        }
    }
}

/// A search result with matching lines from a note.
pub struct SearchMatch {
    /// The note's unique identifier.
    pub note_id: String,

    /// The note's title (as stored by Bear).
    pub title: String,

    /// Groups of line numbers and their content.
    /// Groups are separated by gaps (non-consecutive lines).
    pub groups: Vec<MatchGroup>,
}

/// A contiguous group of matching/context lines.
pub struct MatchGroup {
    pub lines: Vec<(usize, String)>,
}

impl SearchMatch {
    /// Format as pseudo-XML for LLM consumption.
    #[must_use]
    pub fn to_xml(&self) -> String {
        let mut out = format!(
            "<match note-id=\"{}\" title=\"{}\">",
            self.note_id,
            xml_escape(&self.title),
        );

        for (idx, group) in self.groups.iter().enumerate() {
            if idx > 0 {
                out.push_str("\n...");
            }
            out.push('\n');
            for (line_num, text) in &group.lines {
                out.push_str(&format!("{line_num:03}: {text}\n"));
            }
        }

        out.push_str("</match>");
        out
    }
}

/// A note row from the scoring SQL query.
struct ScoredNote {
    id: String,
    title: String,
    content: Option<String>,
    #[allow(dead_code)] // used for ORDER BY in SQL, not read in Rust
    score: f64,
}

/// Execute a search against the Bear database.
///
/// Dispatches to FTS5 or LIKE search based on [`SearchMode`]. In `Auto` mode,
/// FTS5 is attempted first with a fallback to LIKE on error or empty results.
pub fn execute(conn: &Connection, cte: &str, params: &SearchParams) -> Result<Vec<SearchMatch>> {
    let has_queries = params.queries.iter().any(|q| !q.trim().is_empty());
    if !has_queries {
        return execute_like(conn, cte, params);
    }

    match params.mode {
        SearchMode::Like => execute_like(conn, cte, params),
        SearchMode::Fts => execute_fts(conn, cte, params),
        SearchMode::Auto => match execute_fts(conn, cte, params) {
            Ok(results) if !results.is_empty() => Ok(results),
            Ok(_) => {
                tracing::debug!("FTS5 returned no results, falling back to LIKE");
                execute_like(conn, cte, params)
            }
            Err(e) => {
                tracing::debug!(error = %e, "FTS5 search failed, falling back to LIKE");
                execute_like(conn, cte, params)
            }
        },
    }
}

/// FTS5-based search with trigram fallback for substring matching.
fn execute_fts(conn: &Connection, cte: &str, params: &SearchParams) -> Result<Vec<SearchMatch>> {
    let allowed_ids = get_filtered_note_ids(conn, cte, &params.tags, &params.ids)?;

    // Over-fetch when post-filtering by tags/IDs, since some results get
    // removed.
    let fetch_limit = if allowed_ids.is_some() {
        params.limit.saturating_mul(4)
    } else {
        params.limit
    };

    fts::setup_word_table(conn, cte)?;
    let mut fts_results = fts::search_words(conn, &params.queries, fetch_limit)?;

    // Fall back to trigram for substring matching when word search finds
    // nothing.
    if fts_results.is_empty() {
        match fts::setup_trigram_table(conn, cte)
            .and_then(|()| fts::search_trigrams(conn, &params.queries, fetch_limit))
        {
            Ok(trigram_results) => fts_results = trigram_results,
            Err(error) => tracing::debug!(%error, "Trigram fallback failed"),
        }
    }

    if let Some(ref allowed) = allowed_ids {
        fts_results.retain(|r| allowed.contains(&r.note_id));
    }
    fts_results.truncate(params.limit);

    Ok(fts_results
        .into_iter()
        .map(|r| {
            let content = r.content.unwrap_or_default();
            let groups = extract_matching_lines(&content, &params.queries, params.context);

            SearchMatch {
                note_id: r.note_id,
                title: r.title,
                groups,
            }
        })
        .collect())
}

/// Returns the set of note IDs permitted by tag and ID filters.
///
/// Returns `None` when no filtering is needed.
fn get_filtered_note_ids(
    conn: &Connection,
    cte: &str,
    tags: &[String],
    ids: &[String],
) -> Result<Option<HashSet<String>>> {
    if tags.is_empty() && ids.is_empty() {
        return Ok(None);
    }

    rusqlite::vtab::array::load_module(conn)?;

    let mut conditions = vec!["n.is_trashed = 0".to_string()];
    let mut bind_values: Vec<Box<dyn rusqlite::types::ToSql>> = vec![];

    if !tags.is_empty() {
        let values = Rc::new(tags.iter().cloned().map(Value::from).collect::<Vec<_>>());
        bind_values.push(Box::new(values));
        let idx = bind_values.len();
        conditions.push(format!(
            "n.id IN (
                SELECT nt.note_id FROM note_tags nt
                JOIN tags t ON t.id = nt.tag_id
                WHERE t.name IN rarray(?{idx})
                GROUP BY nt.note_id
                HAVING COUNT(DISTINCT t.name) = {}
            )",
            tags.len()
        ));
    }

    if !ids.is_empty() {
        let values = Rc::new(ids.iter().cloned().map(Value::from).collect::<Vec<_>>());
        bind_values.push(Box::new(values));
        let idx = bind_values.len();
        conditions.push(format!("n.id IN rarray(?{idx})"));
    }

    let where_clause = conditions.join(" AND ");
    let sql = format!("{cte} SELECT n.id FROM notes n WHERE {where_clause}");
    let refs: Vec<&dyn rusqlite::types::ToSql> = bind_values.iter().map(AsRef::as_ref).collect();

    let mut stmt = conn.prepare(&sql)?;
    let note_ids = stmt
        .query_map(refs.as_slice(), |row| row.get::<_, String>(0))?
        .collect::<std::result::Result<_, _>>()?;

    Ok(Some(note_ids))
}

/// LIKE-based search (original implementation).
///
/// Results are ranked by a hand-rolled scoring formula:
/// exact title match = 1.0, title LIKE = 0.5, content LIKE = 0.1.
#[allow(clippy::too_many_lines)]
fn execute_like(conn: &Connection, cte: &str, params: &SearchParams) -> Result<Vec<SearchMatch>> {
    rusqlite::vtab::array::load_module(conn)?;

    let mut conditions = vec!["n.is_trashed = 0".to_string()];
    let mut bind_values: Vec<Box<dyn rusqlite::types::ToSql>> = vec![];

    // Tag filter
    if !params.tags.is_empty() {
        let values = Rc::new(
            params
                .tags
                .iter()
                .cloned()
                .map(Value::from)
                .collect::<Vec<_>>(),
        );
        bind_values.push(Box::new(values));
        let idx = bind_values.len();
        conditions.push(format!(
            "n.id IN (
                SELECT nt.note_id FROM note_tags nt
                JOIN tags t ON t.id = nt.tag_id
                WHERE t.name IN rarray(?{idx})
                GROUP BY nt.note_id
                HAVING COUNT(DISTINCT t.name) = {}
            )",
            params.tags.len()
        ));
    }

    // ID filter
    if !params.ids.is_empty() {
        let values = Rc::new(
            params
                .ids
                .iter()
                .cloned()
                .map(Value::from)
                .collect::<Vec<_>>(),
        );
        bind_values.push(Box::new(values));
        let idx = bind_values.len();
        conditions.push(format!("n.id IN rarray(?{idx})"));
    }

    // Build scoring expression and WHERE filter for text queries.
    // Each query contributes a score:
    //   exact title match  = 1.0
    //   title LIKE match   = 0.5
    //   content LIKE match = 0.1
    //
    // All queries must match somewhere (AND), but the score is the sum.
    let mut score_terms = vec![];

    for query in &params.queries {
        // Exact match bind value
        bind_values.push(Box::new(query.clone()));
        let exact_idx = bind_values.len();

        // LIKE pattern bind value
        let pat = format!("%{query}%");
        bind_values.push(Box::new(pat));
        let like_idx = bind_values.len();

        // Each query must match title or content
        conditions.push(format!(
            "(n.title LIKE ?{like_idx} OR n.content LIKE ?{like_idx})"
        ));

        // Score contribution for this query
        score_terms.push(format!(
            "(CASE
                WHEN n.title = ?{exact_idx} THEN 1.0
                WHEN n.title LIKE ?{like_idx} THEN 0.5
                WHEN n.content LIKE ?{like_idx} THEN 0.1
                ELSE 0.0
            END)"
        ));
    }

    let score_expr = if score_terms.is_empty() {
        "1.0".to_string()
    } else {
        score_terms.join(" + ")
    };

    let where_clause = conditions.join(" AND ");
    let sql = format!(
        "{cte}
         SELECT n.id, n.title, n.content, ({score_expr}) AS score
         FROM notes n
         WHERE {where_clause}
         ORDER BY score DESC
         LIMIT {limit}",
        limit = params.limit,
    );

    let refs: Vec<&dyn rusqlite::types::ToSql> = bind_values.iter().map(AsRef::as_ref).collect();

    let mut stmt = conn.prepare(&sql)?;
    let scored_notes: Vec<ScoredNote> = stmt
        .query_map(refs.as_slice(), |row| {
            Ok(ScoredNote {
                id: row.get(0)?,
                title: row.get(1)?,
                content: row.get(2)?,
                score: row.get(3)?,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;

    let mut matches = vec![];
    for note in scored_notes {
        let content = note.content.unwrap_or_default();
        let groups = extract_matching_lines(&content, &params.queries, params.context);

        matches.push(SearchMatch {
            note_id: note.id,
            title: note.title,
            groups,
        });
    }

    // Secondary sort: within the same SQL score tier, notes with more
    // content-level line hits come first.
    // (SQL already orders by score DESC, so this is a stable tiebreaker.)
    matches.sort_by(|a, b| {
        let a_lines: usize = a.groups.iter().map(|g| g.lines.len()).sum();
        let b_lines: usize = b.groups.iter().map(|g| g.lines.len()).sum();
        b_lines.cmp(&a_lines)
    });

    Ok(matches)
}

/// Find matching lines in content and group them with context.
fn extract_matching_lines(content: &str, queries: &[String], context: usize) -> Vec<MatchGroup> {
    let lines: Vec<&str> = content.lines().collect();

    // Find lines matching any query
    let mut hit_lines = BTreeSet::new();
    for query in queries {
        let lower_query = query.to_lowercase();
        for (idx, line) in lines.iter().enumerate() {
            if line.to_lowercase().contains(&lower_query) {
                hit_lines.insert(idx);
            }
        }
    }

    if hit_lines.is_empty() {
        // Title-only match; show first few lines as preview
        let end = lines.len().min(context * 2 + 1);
        if end == 0 {
            return vec![];
        }
        let group_lines = (0..end).map(|i| (i + 1, lines[i].to_string())).collect();
        return vec![MatchGroup { lines: group_lines }];
    }

    // Expand hits with context
    let mut visible = BTreeSet::new();
    for &hit in &hit_lines {
        let start = hit.saturating_sub(context);
        let end = (hit + context + 1).min(lines.len());
        for i in start..end {
            visible.insert(i);
        }
    }

    // Group consecutive lines
    let mut groups = vec![];
    let mut current_group: Vec<(usize, String)> = vec![];
    let mut prev: Option<usize> = None;

    for &idx in &visible {
        if let Some(p) = prev
            && idx != p + 1
            && !current_group.is_empty()
        {
            groups.push(MatchGroup {
                lines: std::mem::take(&mut current_group),
            });
        }
        current_group.push((idx + 1, lines[idx].to_string())); // 1-indexed
        prev = Some(idx);
    }
    if !current_group.is_empty() {
        groups.push(MatchGroup {
            lines: current_group,
        });
    }

    groups
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
#[path = "search_tests.rs"]
mod tests;
