use std::{
    collections::{HashMap, HashSet},
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

    /// Maximum number of notes to return (default: 50).
    pub limit: usize,

    /// Search backend to use.
    pub mode: SearchMode,

    /// Maximum characters in each result's snippet text (default: 200).
    ///
    /// Longer matching lines are truncated with `…` and centered on the
    /// match. Callers should use `note_get` with the returned line numbers
    /// to fetch full content when the snippet is insufficient.
    pub snippet_chars: usize,

    /// Maximum number of line numbers reported in `SearchMatch::line_hits`
    /// (default: 20).
    ///
    /// `SearchMatch::total_hits` always reports the true count, so callers
    /// know when this cap kicked in.
    pub max_line_hits: usize,
}

impl Default for SearchParams {
    fn default() -> Self {
        Self {
            queries: vec![],
            tags: vec![],
            ids: vec![],
            limit: 50,
            mode: SearchMode::default(),
            snippet_chars: 200,
            max_line_hits: 20,
        }
    }
}

impl SearchParams {
    /// Return a copy with glob-style wildcard queries (`"*"`) removed.
    ///
    /// Users pass `queries: ["*"]` to mean "match everything" (e.g. when
    /// filtering by tags only), but `*` is not a wildcard in SQL LIKE or
    /// FTS5. Stripping it makes the search fall through to the tag/ID-only
    /// path.
    fn without_wildcards(&self) -> Self {
        Self {
            queries: self
                .queries
                .iter()
                .filter(|q| q.trim() != "*")
                .cloned()
                .collect(),
            tags: self.tags.clone(),
            ids: self.ids.clone(),
            limit: self.limit,
            mode: self.mode,
            snippet_chars: self.snippet_chars,
            max_line_hits: self.max_line_hits,
        }
    }
}

/// A bounded summary of a matching note.
///
/// Carries metadata plus a short snippet showing why the note matched. To
/// read full content, the caller should follow up with `note_get`, passing
/// the values from `line_hits` (or ranges around them) via its `lines`
/// parameter.
pub struct SearchMatch {
    /// The note's unique identifier.
    pub note_id: String,

    /// The note's title (as stored by Bear).
    pub title: String,

    /// The note's tags.
    pub tags: Vec<String>,

    /// When the note was last modified, if known.
    pub updated_at: Option<String>,

    /// 1-indexed line numbers in the note's content where the query matched.
    ///
    /// Capped at `SearchParams::max_line_hits`; check `total_hits` for the
    /// true count. Empty for title-only matches.
    pub line_hits: Vec<usize>,

    /// Total number of content lines that matched the query.
    ///
    /// May exceed `line_hits.len()` when capped.
    pub total_hits: usize,

    /// A short excerpt centered on the first match.
    ///
    /// `None` only when the note has no content at all.
    pub snippet: Option<Snippet>,
}

/// A short text excerpt with the line it came from.
pub struct Snippet {
    /// 1-indexed source line.
    pub line: usize,

    /// Excerpt text. Prefixed/suffixed with `…` when truncated.
    pub text: String,
}

impl SearchMatch {
    /// Format as pseudo-XML for LLM consumption.
    ///
    /// The output is intentionally compact and size-bounded. To read full
    /// content, the caller should follow up with `note_get`, passing
    /// `line_hits` (or a range around them) via its `lines` parameter.
    #[must_use]
    pub fn to_xml(&self) -> String {
        let mut out = format!(
            "<match note-id=\"{}\" title=\"{}\" tags=\"{}\" updated-at=\"{}\" total-hits=\"{}\">",
            xml_escape(&self.note_id),
            xml_escape(&self.title),
            xml_escape(&self.tags.join(" ")),
            xml_escape(self.updated_at.as_deref().unwrap_or("unknown")),
            self.total_hits,
        );

        if let Some(snippet) = &self.snippet {
            out.push_str(&format!(
                "\n  <snippet line=\"{}\">{}</snippet>",
                snippet.line,
                xml_escape(&snippet.text),
            ));
        }

        if !self.line_hits.is_empty() {
            let hits = self
                .line_hits
                .iter()
                .map(usize::to_string)
                .collect::<Vec<_>>()
                .join(", ");
            out.push_str(&format!("\n  <hits>{hits}</hits>"));
        }

        out.push_str("\n</match>");
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
    // Treat "*" as a match-all wildcard (glob convention), not a literal.
    let params = params.without_wildcards();

    let has_queries = params.queries.iter().any(|q| !q.trim().is_empty());
    if !has_queries {
        return execute_like(conn, cte, &params);
    }

    match params.mode {
        SearchMode::Like => execute_like(conn, cte, &params),
        SearchMode::Fts => execute_fts(conn, cte, &params),
        SearchMode::Auto => match execute_fts(conn, cte, &params) {
            Ok(results) if !results.is_empty() => Ok(results),
            Ok(_) => {
                tracing::debug!("FTS5 returned no results, falling back to LIKE");
                execute_like(conn, cte, &params)
            }
            Err(e) => {
                tracing::debug!(error = %e, "FTS5 search failed, falling back to LIKE");
                execute_like(conn, cte, &params)
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

    let note_ids: Vec<String> = fts_results.iter().map(|r| r.note_id.clone()).collect();
    let meta = fetch_metadata(conn, cte, &note_ids)?;

    Ok(fts_results
        .into_iter()
        .map(|r| {
            let content = r.content.unwrap_or_default();
            let (line_hits, total_hits, snippet) = extract_hits_and_snippet(
                &content,
                &params.queries,
                params.max_line_hits,
                params.snippet_chars,
            );

            let m = meta.get(&r.note_id);
            SearchMatch {
                note_id: r.note_id,
                title: r.title,
                tags: m.map(|m| m.tags.clone()).unwrap_or_default(),
                updated_at: m.and_then(|m| m.updated_at.clone()),
                line_hits,
                total_hits,
                snippet,
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

    let note_ids: Vec<String> = scored_notes.iter().map(|n| n.id.clone()).collect();
    let meta = fetch_metadata(conn, cte, &note_ids)?;

    let mut matches = vec![];
    for note in scored_notes {
        let content = note.content.unwrap_or_default();
        let (line_hits, total_hits, snippet) = extract_hits_and_snippet(
            &content,
            &params.queries,
            params.max_line_hits,
            params.snippet_chars,
        );

        let m = meta.get(&note.id);
        matches.push(SearchMatch {
            note_id: note.id,
            title: note.title,
            tags: m.map(|m| m.tags.clone()).unwrap_or_default(),
            updated_at: m.and_then(|m| m.updated_at.clone()),
            line_hits,
            total_hits,
            snippet,
        });
    }

    // Secondary sort: within the same SQL score tier, notes with more
    // content-level hits come first.
    // (SQL already orders by score DESC, so this is a stable tiebreaker.)
    matches.sort_by_key(|m| std::cmp::Reverse(m.total_hits));

    Ok(matches)
}

/// Find matching lines and produce a snippet showing the best match.
///
/// Returns `(line_hits, total_hits, snippet)`. `line_hits` is truncated to
/// `max_line_hits`; `total_hits` is always the full count. When no content
/// line matches the query (title-only match), `line_hits` is empty and the
/// snippet previews the first non-empty content line. `snippet` is `None`
/// only when the note has no content at all.
fn extract_hits_and_snippet(
    content: &str,
    queries: &[String],
    max_line_hits: usize,
    snippet_chars: usize,
) -> (Vec<usize>, usize, Option<Snippet>) {
    let lines: Vec<&str> = content.lines().collect();
    let lowered_queries: Vec<String> = queries
        .iter()
        .filter(|q| !q.trim().is_empty())
        .map(|q| q.to_lowercase())
        .collect();

    let mut hits: Vec<usize> = vec![];
    let mut first_hit: Option<(usize, usize)> = None; // (line_idx, byte_pos)

    if !lowered_queries.is_empty() {
        for (idx, line) in lines.iter().enumerate() {
            let lowered = line.to_lowercase();
            let mut earliest: Option<usize> = None;
            for q in &lowered_queries {
                if let Some(pos) = lowered.find(q) {
                    earliest = Some(earliest.map_or(pos, |p| p.min(pos)));
                }
            }

            if let Some(pos) = earliest {
                hits.push(idx + 1); // 1-indexed
                if first_hit.is_none() {
                    first_hit = Some((idx, pos));
                }
            }
        }
    }

    let total_hits = hits.len();

    let snippet = if let Some((line_idx, match_pos)) = first_hit {
        Some(Snippet {
            line: line_idx + 1,
            text: make_snippet(lines[line_idx], match_pos, snippet_chars),
        })
    } else {
        // Title-only match (or empty query): preview the first non-empty line.
        lines
            .iter()
            .enumerate()
            .find(|(_, l)| !l.trim().is_empty())
            .map(|(idx, line)| Snippet {
                line: idx + 1,
                text: make_snippet(line, 0, snippet_chars),
            })
    };

    if hits.len() > max_line_hits {
        hits.truncate(max_line_hits);
    }

    (hits, total_hits, snippet)
}

/// Truncate `line` to roughly `max_chars` characters, centered on
/// `match_byte_pos`. Prefixes and/or suffixes the result with `…` when
/// truncation actually happened.
///
/// `match_byte_pos` is a hint and is clamped to the line's byte length, so
/// callers can safely pass approximate positions derived from a lower-cased
/// copy of the line.
fn make_snippet(line: &str, match_byte_pos: usize, max_chars: usize) -> String {
    let char_count = line.chars().count();
    if char_count <= max_chars {
        return line.to_string();
    }

    let half = max_chars / 2;
    let clamped_pos = match_byte_pos.min(line.len());
    let match_char_pos = line[..clamped_pos].chars().count();

    let mut start_char = match_char_pos.saturating_sub(half);
    let end_char = (start_char + max_chars).min(char_count);
    start_char = end_char.saturating_sub(max_chars);

    let start_byte = line.char_indices().nth(start_char).map_or(0, |(b, _)| b);
    let end_byte = line
        .char_indices()
        .nth(end_char)
        .map_or(line.len(), |(b, _)| b);

    let mut out = String::with_capacity(end_byte - start_byte + 8);
    if start_char > 0 {
        out.push('…');
    }
    out.push_str(&line[start_byte..end_byte]);
    if end_char < char_count {
        out.push('…');
    }
    out
}

struct NoteMeta {
    tags: Vec<String>,
    updated_at: Option<String>,
}

/// Fetch tags and `updated_at` for a batch of note IDs in one query.
fn fetch_metadata(
    conn: &Connection,
    cte: &str,
    note_ids: &[String],
) -> Result<HashMap<String, NoteMeta>> {
    if note_ids.is_empty() {
        return Ok(HashMap::new());
    }

    rusqlite::vtab::array::load_module(conn)?;

    let values = Rc::new(
        note_ids
            .iter()
            .cloned()
            .map(Value::from)
            .collect::<Vec<_>>(),
    );
    let bind: Vec<Box<dyn rusqlite::types::ToSql>> = vec![Box::new(values)];
    let refs: Vec<&dyn rusqlite::types::ToSql> = bind.iter().map(AsRef::as_ref).collect();

    let sql = format!(
        "{cte}
         SELECT n.id, n.updated_at, t.name
         FROM notes n
         LEFT JOIN note_tags nt ON nt.note_id = n.id
         LEFT JOIN tags t ON t.id = nt.tag_id
         WHERE n.id IN rarray(?1)
         ORDER BY n.id, t.name"
    );

    let mut stmt = conn.prepare(&sql)?;
    let mut out: HashMap<String, NoteMeta> = HashMap::new();

    let rows = stmt.query_map(refs.as_slice(), |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, Option<String>>(1)?,
            row.get::<_, Option<String>>(2)?,
        ))
    })?;

    for row in rows {
        let (id, updated_at, tag) = row?;
        let entry = out.entry(id).or_insert(NoteMeta {
            tags: vec![],
            updated_at,
        });
        if let Some(t) = tag {
            entry.tags.push(t);
        }
    }

    Ok(out)
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
