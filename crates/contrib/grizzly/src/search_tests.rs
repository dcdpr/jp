use super::*;
use crate::{BearDb, schema};

/// Notes for [`a_date_filter_is_applied_before_the_fts_limit`]: how many times
/// each repeats the search term, and the day it was created.
///
/// Rank follows the repeat count, so the order is fixed rather than left to
/// BM25 ties among identical documents.
/// Only the two created on day 9 are inside the cutoff, and they sit either
/// side of the eighth row — which is where a `4 * limit` fetch would have
/// stopped looking.
const BOUNDARY_NOTES: [(usize, i64); 10] = [
    (10, 9),
    (9, 0),
    (8, 0),
    (7, 0),
    (6, 0),
    (5, 0),
    (4, 0),
    (3, 0),
    (2, 9),
    (1, 0),
];

/// Build a database from [`BOUNDARY_NOTES`], returning a connection and its
/// normalizing CTE.
///
/// Its own fixture rather than the shared one: this needs more notes than `4 *
/// limit`, and growing `setup_test_schema` would move every count the other
/// tests assert.
fn boundary_corpus() -> (rusqlite::Connection, String) {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE TABLE ZSFNOTE (
            Z_PK INTEGER PRIMARY KEY,
            ZUNIQUEIDENTIFIER TEXT,
            ZTITLE TEXT,
            ZTEXT TEXT,
            ZMODIFICATIONDATE REAL,
            ZCREATIONDATE REAL,
            ZTRASHED INTEGER,
            ZARCHIVED INTEGER
         );
         CREATE TABLE ZSFNOTETAG (Z_PK INTEGER PRIMARY KEY, ZTITLE TEXT);
         INSERT INTO ZSFNOTETAG (Z_PK, ZTITLE) VALUES (1, 'topic');
         CREATE TABLE Z_5TAGS (Z_5NOTES INTEGER, Z_13TAGS INTEGER);",
    )
    .unwrap();

    for (index, (repeats, day)) in BOUNDARY_NOTES.iter().enumerate() {
        let pk = i64::try_from(index).unwrap() + 1;
        conn.execute(
            "INSERT INTO ZSFNOTE
                (Z_PK, ZUNIQUEIDENTIFIER, ZTITLE, ZTEXT, ZMODIFICATIONDATE, ZCREATIONDATE,
                 ZTRASHED, ZARCHIVED)
             VALUES (?1, ?2, ?3, ?4, 0, ?5, 0, 0)",
            rusqlite::params![
                pk,
                format!("note-{pk}"),
                format!("Note {pk}"),
                vec!["recurring"; *repeats].join(" "),
                day * 86400,
            ],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO Z_5TAGS (Z_5NOTES, Z_13TAGS) VALUES (?1, 1)",
            rusqlite::params![pk],
        )
        .unwrap();
    }

    let meta = schema::discover(&conn).unwrap();
    let cte = schema::normalizing_cte(&meta);

    (conn, cte)
}

fn search(db: &BearDb, queries: Vec<&str>) -> Vec<SearchMatch> {
    db.search(&SearchParams {
        queries: queries.into_iter().map(Into::into).collect(),
        ..Default::default()
    })
    .unwrap()
}

fn search_with(db: &BearDb, params: &SearchParams) -> Vec<SearchMatch> {
    db.search(params).unwrap()
}

/// Both shapes SQLite renders, and nothing else.
#[test]
fn a_cutoff_takes_a_day_or_an_instant() {
    assert_eq!(
        "2026-08-01".parse::<Cutoff>().unwrap().to_string(),
        "2026-08-01"
    );
    assert_eq!(
        "2026-08-01 13:45:02".parse::<Cutoff>().unwrap().to_string(),
        "2026-08-01 13:45:02"
    );
    assert_eq!(
        "  2026-08-01  ".parse::<Cutoff>().unwrap().to_string(),
        "2026-08-01"
    );
}

/// The comparison is lexical, so an unpadded date is not a near-miss: it means
/// a different day and still returns a plausible answer.
///
/// `2026-08-1` sorts above every `2026-08-09 ...` and below every `2026-08-10
/// ...`, quietly dropping the first nine days of the month.
#[test]
fn a_cutoff_that_would_mean_another_day_is_refused() {
    for input in [
        "2026-08-1",
        "2026-8-01",
        "26-08-01",
        "2026/08/01",
        "2026-08-01T00:00:00",
        "2026-08-01 13:45",
        "yesterday",
        "",
    ] {
        assert!(
            input.parse::<Cutoff>().is_err(),
            "`{input}` should not parse as a cutoff"
        );
    }

    assert_eq!(
        "2026-08-1".parse::<Cutoff>().unwrap_err().to_string(),
        "`2026-08-1` is not a date; write `YYYY-MM-DD` or `YYYY-MM-DD HH:MM:SS`, zero-padded."
    );
}

#[test]
fn search_by_content() {
    let db = BearDb::in_memory().unwrap();
    let results = search(&db, vec!["productivity"]);

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].note_id, "note-1");
    assert_eq!(results[0].title, "Getting Things Done");
}

#[test]
fn search_with_tag_filter() {
    let db = BearDb::in_memory().unwrap();
    let results = search_with(&db, &SearchParams {
        queries: vec!["25-minute".into()],
        tags: vec!["productivity".into()],
        ..Default::default()
    });

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].note_id, "note-2");
}

#[test]
fn search_with_tag_filter_no_match() {
    let db = BearDb::in_memory().unwrap();
    let results = search_with(&db, &SearchParams {
        queries: vec!["productivity".into()],
        tags: vec!["personal".into()],
        ..Default::default()
    });

    assert!(results.is_empty());
}

#[test]
fn line_hits_reports_matching_line_numbers() {
    let db = BearDb::in_memory().unwrap();
    let results = search_with(&db, &SearchParams {
        queries: vec!["capturing".into()],
        ..Default::default()
    });

    // "capturing" is on line 2 of note-1
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].line_hits, vec![2]);
    assert_eq!(results[0].total_hits, 1);
    assert_eq!(results[0].snippet.as_ref().unwrap().line, 2);
}

#[test]
fn search_excludes_trashed() {
    let db = BearDb::in_memory().unwrap();
    let results = search(&db, vec!["trashed"]);
    assert!(results.is_empty());
}

#[test]
fn search_includes_archived_but_suppresses_content() {
    let db = BearDb::in_memory().unwrap();
    let results = search(&db, vec!["archivedterm"]);

    assert_eq!(results.len(), 1);
    let m = &results[0];
    assert_eq!(m.note_id, "note-5");
    assert!(m.is_archived);
    // The note stays discoverable: title and the true hit count are reported.
    assert_eq!(m.title, "Archived Note");
    assert_eq!(m.total_hits, 2);
    // But none of its content is rendered.
    assert!(m.line_hits.is_empty());
    assert!(m.snippet.is_none());
}

#[test]
fn archived_xml_marks_archived_and_omits_content() {
    let m = SearchMatch {
        note_id: "arch-1".into(),
        title: "Archived".into(),
        tags: vec![],
        updated_at: None,
        created_at: None,
        line_hits: vec![],
        total_hits: 3,
        snippet: None,
        is_archived: true,
    };

    let xml = m.to_xml();
    // A note whose creation date didn't come back still reports the attribute,
    // so a caller never has to tell "absent" from "unknown".
    assert!(xml.contains(r#"created-at="unknown""#));
    assert!(xml.contains(r#"archived="true""#));
    assert!(xml.contains(r#"total-hits="3""#));
    assert!(!xml.contains("<snippet"));
    assert!(!xml.contains("<hits>"));
}

#[test]
fn title_exact_match_ranks_first() {
    let db = BearDb::in_memory().unwrap();
    // "Shopping List" is both a title and appears nowhere else.
    // "Pomodoro Technique" is a title.
    // Search for a term that matches a title exactly vs content.
    let results = search(&db, vec!["Pomodoro Technique"]);

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].note_id, "note-2");
}

#[test]
fn title_match_ranks_above_content_match() {
    let db = BearDb::in_memory().unwrap();
    // "productivity" appears in note-1 title ("Getting Things Done" doesn't
    // contain it) but IS in the content. Actually, let's search for
    // something that matches one note's title and another's content.
    // "Shopping" matches note-3 title. It doesn't appear in other notes.
    // Instead, let's test with a broader term.
    //
    // note-1: title="Getting Things Done", content has "productivity"
    // note-2: title="Pomodoro Technique", content has "25-minute intervals"
    // note-3: title="Shopping List", content has "Eggs\nMilk\nBread"
    //
    // Searching "Shopping" should return note-3 first (title LIKE match).
    let results = search(&db, vec!["Shopping"]);

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].note_id, "note-3");
}

#[test]
fn result_limit() {
    let db = BearDb::in_memory().unwrap();
    let results = search_with(&db, &SearchParams {
        queries: vec!["e".into()], // broad query, matches multiple notes
        limit: 1,
        ..Default::default()
    });

    assert_eq!(results.len(), 1);
}

#[test]
fn title_in_xml_output() {
    let m = SearchMatch {
        note_id: "abc-123".into(),
        title: "My Note".into(),
        tags: vec!["work".into()],
        updated_at: Some("2024-01-01 00:00:00".into()),
        created_at: Some("2024-01-01 00:00:00".into()),
        line_hits: vec![1],
        total_hits: 1,
        snippet: Some(Snippet {
            line: 1,
            text: "first line".into(),
        }),
        is_archived: false,
    };

    let xml = m.to_xml();
    assert!(xml.contains(r#"note-id="abc-123""#));
    assert!(xml.contains(r#"title="My Note""#));
    assert!(xml.contains(r#"tags="work""#));
    assert!(xml.contains(r#"created-at="2024-01-01 00:00:00""#));
    assert!(xml.contains(r#"updated-at="2024-01-01 00:00:00""#));
    assert!(xml.contains(r#"total-hits="1""#));
    assert!(xml.contains(r#"<snippet line="1">first line</snippet>"#));
    assert!(xml.contains("<hits>1</hits>"));
}

#[test]
fn title_xml_escaping() {
    let m = SearchMatch {
        note_id: "x".into(),
        title: r#"Notes & "Quotes" <stuff>"#.into(),
        tags: vec![],
        updated_at: None,
        created_at: None,
        line_hits: vec![],
        total_hits: 0,
        snippet: None,
        is_archived: false,
    };

    let xml = m.to_xml();
    assert!(xml.contains(r#"title="Notes &amp; &quot;Quotes&quot; &lt;stuff&gt;""#));
}

#[test]
fn xml_lists_all_line_hits() {
    let m = SearchMatch {
        note_id: "abc-123".into(),
        title: "Test".into(),
        tags: vec![],
        updated_at: None,
        created_at: None,
        line_hits: vec![10, 11, 50],
        total_hits: 3,
        snippet: Some(Snippet {
            line: 10,
            text: "line ten".into(),
        }),
        is_archived: false,
    };

    let xml = m.to_xml();
    assert!(xml.contains("<hits>10, 11, 50</hits>"));
    assert!(xml.contains(r#"<snippet line="10">line ten</snippet>"#));
}

#[test]
fn fts_mode_word_search() {
    let db = BearDb::in_memory().unwrap();
    let results = search_with(&db, &SearchParams {
        queries: vec!["productivity".into()],
        mode: SearchMode::Fts,
        ..Default::default()
    });
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].note_id, "note-1");
}

#[test]
fn like_mode_substring() {
    let db = BearDb::in_memory().unwrap();
    // "prod" is a substring, not a full word — LIKE matches, FTS5 wouldn't
    let results = search_with(&db, &SearchParams {
        queries: vec!["prod".into()],
        mode: SearchMode::Like,
        ..Default::default()
    });
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].note_id, "note-1");
}

#[test]
fn fts_mode_with_tag_filter() {
    let db = BearDb::in_memory().unwrap();
    let results = search_with(&db, &SearchParams {
        queries: vec!["intervals".into()],
        tags: vec!["productivity".into()],
        mode: SearchMode::Fts,
        ..Default::default()
    });
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].note_id, "note-2");
}

#[test]
fn fts_mode_tag_filter_excludes() {
    let db = BearDb::in_memory().unwrap();
    // "productivity" matches note-1 content, but note-1 isn't tagged "personal"
    let results = search_with(&db, &SearchParams {
        queries: vec!["productivity".into()],
        tags: vec!["personal".into()],
        mode: SearchMode::Fts,
        ..Default::default()
    });
    assert!(results.is_empty());
}

#[test]
fn auto_falls_back_to_like_for_short_queries() {
    let db = BearDb::in_memory().unwrap();
    // "pr" is too short for FTS5 word match (no standalone word "pr")
    // and too short for trigram (< 3 chars), so Auto falls back to LIKE
    let results = search_with(&db, &SearchParams {
        queries: vec!["pr".into()],
        ..Default::default()
    });
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].note_id, "note-1");
}

#[test]
fn auto_uses_fts_when_available() {
    let db = BearDb::in_memory().unwrap();
    // "productivity" is a full word — FTS5 should handle it directly
    let results = search_with(&db, &SearchParams {
        queries: vec!["productivity".into()],
        ..Default::default()
    });
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].note_id, "note-1");
}

#[test]
fn wildcard_query_with_tag_filter() {
    let db = BearDb::in_memory().unwrap();
    let results = search_with(&db, &SearchParams {
        queries: vec!["*".into()],
        tags: vec!["productivity".into()],
        ..Default::default()
    });
    assert_eq!(results.len(), 2);

    let ids: Vec<&str> = results.iter().map(|r| r.note_id.as_str()).collect();
    assert!(ids.contains(&"note-1"));
    assert!(ids.contains(&"note-2"));
}

#[test]
fn wildcard_query_with_nested_tag_filter() {
    let db = BearDb::in_memory().unwrap();
    let results = search_with(&db, &SearchParams {
        queries: vec!["*".into()],
        tags: vec!["projects/jp".into()],
        ..Default::default()
    });
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].note_id, "note-1");
}

#[test]
fn nested_tag_filter_with_content_query() {
    let db = BearDb::in_memory().unwrap();
    let results = search_with(&db, &SearchParams {
        queries: vec!["capturing".into()],
        tags: vec!["projects/jp".into()],
        ..Default::default()
    });
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].note_id, "note-1");
}

#[test]
fn nested_tag_filter_excludes_untagged() {
    let db = BearDb::in_memory().unwrap();
    // note-2 has "intervals" but is NOT tagged projects/jp
    let results = search_with(&db, &SearchParams {
        queries: vec!["intervals".into()],
        tags: vec!["projects/jp".into()],
        ..Default::default()
    });
    assert!(results.is_empty());
}

#[test]
fn result_carries_tags_and_timestamps() {
    let db = BearDb::in_memory().unwrap();
    let results = search(&db, vec!["productivity"]);

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].note_id, "note-1");
    assert_eq!(results[0].tags, vec!["productivity", "projects/jp"]);
    assert!(results[0].updated_at.is_some());
    assert_eq!(
        results[0].created_at.as_deref(),
        Some("2001-01-01 00:00:00")
    );
}

/// note-1 and note-2 both carry `productivity`, a day apart.
#[test]
fn created_after_drops_notes_older_than_the_cutoff() {
    let db = BearDb::in_memory().unwrap();
    let results = search_with(&db, &SearchParams {
        queries: vec!["*".into()],
        tags: vec!["productivity".into()],
        created_after: Some("2001-01-02".parse().unwrap()),
        ..Default::default()
    });

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].note_id, "note-2");
}

/// "On or after": a cutoff equal to a note's creation instant keeps it.
///
/// Spelled as a full timestamp rather than a bare day, because a bare day is a
/// prefix of every time on it and so cannot tell `>=` from `>`.
#[test]
fn created_after_keeps_a_note_created_exactly_on_the_cutoff() {
    let db = BearDb::in_memory().unwrap();
    let results = search_with(&db, &SearchParams {
        queries: vec!["*".into()],
        created_after: Some("2001-01-03 00:00:00".parse().unwrap()),
        ..Default::default()
    });

    assert_eq!(
        results
            .iter()
            .map(|r| r.note_id.as_str())
            .collect::<Vec<_>>(),
        vec!["note-3"],
        "note-3 was created at exactly this instant"
    );
}

#[test]
fn created_after_beyond_every_note_matches_nothing() {
    let db = BearDb::in_memory().unwrap();
    let results = search_with(&db, &SearchParams {
        queries: vec!["*".into()],
        created_after: Some("2026-01-01".parse().unwrap()),
        ..Default::default()
    });

    assert!(results.is_empty());
}

/// A date cutoff and FTS relevance are uncorrelated, so the excluded notes can
/// fill a whole page on their own.
///
/// Filtering after the fetch leaves one survivor here, and one survivor is
/// non-empty — which is enough to stop `Auto` falling back to LIKE, so the
/// eligible note below the boundary is never reported at all.
#[test]
fn a_date_filter_is_applied_before_the_fts_limit() {
    let (conn, cte) = boundary_corpus();

    let results = execute(&conn, &cte, &SearchParams {
        queries: vec!["recurring".into()],
        created_after: Some("2001-01-10".parse().unwrap()),
        limit: 2,
        ..Default::default()
    })
    .unwrap();

    // note-1 ranks first and note-9 ninth, so a `4 * limit` fetch would have
    // returned note-1 alone and called it a complete answer.
    assert_eq!(
        results
            .iter()
            .map(|r| r.note_id.as_str())
            .collect::<Vec<_>>(),
        vec!["note-1", "note-9"]
    );
}

/// Tag-only queries all score the same, so the date decides the order and a
/// limit takes the newest rather than an arbitrary row.
///
/// Scoped to queryless searches on purpose: with a text query every note also
/// carries a hit count, and the Rust sort below `execute_like` puts that ahead
/// of anything SQL ordered by.
///
/// The cutoff keeps only the two notes with distinct creation dates, so the
/// expected order is fully determined.
#[test]
fn queryless_results_come_back_newest_first() {
    let db = BearDb::in_memory().unwrap();
    let results = search_with(&db, &SearchParams {
        queries: vec!["*".into()],
        created_after: Some("2001-01-02".parse().unwrap()),
        ..Default::default()
    });

    let ids: Vec<&str> = results.iter().map(|r| r.note_id.as_str()).collect();
    assert_eq!(ids, vec!["note-3", "note-2"]);
}

#[test]
fn title_only_match_previews_first_content_line() {
    // "Pomodoro" appears in the title of note-2 but not in its content.
    // Bear notes typically embed the title in the first content line too,
    // so test data: query for "Shopping" which matches note-3 title; first
    // content line is "Eggs".
    let db = BearDb::in_memory().unwrap();
    let results = search(&db, vec!["Shopping"]);

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].note_id, "note-3");
    assert_eq!(results[0].total_hits, 0);
    assert!(results[0].line_hits.is_empty());
    let snippet = results[0]
        .snippet
        .as_ref()
        .expect("snippet for content note");
    assert_eq!(snippet.line, 1);
    assert_eq!(snippet.text, "Eggs");
}

#[test]
fn extract_caps_line_hits_but_reports_full_total() {
    let content = (1..=100)
        .map(|n| format!("line {n} target"))
        .collect::<Vec<_>>()
        .join("\n");

    let (line_hits, total_hits, snippet) =
        extract_hits_and_snippet(&content, &["target".into()], 5, 200);

    assert_eq!(line_hits.len(), 5);
    assert_eq!(line_hits, vec![1, 2, 3, 4, 5]);
    assert_eq!(total_hits, 100);
    assert_eq!(snippet.unwrap().line, 1);
}

#[test]
fn extract_returns_no_snippet_for_empty_content() {
    let (line_hits, total_hits, snippet) =
        extract_hits_and_snippet("", &["anything".into()], 20, 200);

    assert!(line_hits.is_empty());
    assert_eq!(total_hits, 0);
    assert!(snippet.is_none());
}

#[test]
fn make_snippet_short_line_returned_unchanged() {
    let line = "hello world";
    assert_eq!(make_snippet(line, 0, 200), "hello world");
}

#[test]
fn make_snippet_truncates_long_line_with_ellipses() {
    // 1000 'a' chars with the match at position 500.
    let line: String = "a".repeat(1000);
    let snippet = make_snippet(&line, 500, 50);

    // Leading + trailing ellipsis, ~50 chars in between.
    assert!(snippet.starts_with('\u{2026}'));
    assert!(snippet.ends_with('\u{2026}'));
    assert_eq!(snippet.chars().count(), 52); // 50 + 2 ellipses
}

#[test]
fn make_snippet_at_line_start_no_leading_ellipsis() {
    let line: String = "a".repeat(1000);
    let snippet = make_snippet(&line, 0, 50);

    assert!(!snippet.starts_with('\u{2026}'));
    assert!(snippet.ends_with('\u{2026}'));
}

#[test]
fn make_snippet_at_line_end_no_trailing_ellipsis() {
    let line: String = "a".repeat(1000);
    let snippet = make_snippet(&line, 1000, 50);

    assert!(snippet.starts_with('\u{2026}'));
    assert!(!snippet.ends_with('\u{2026}'));
}

#[test]
fn make_snippet_handles_multibyte_chars() {
    // 1000 é chars (2 bytes each). Each is one char but two bytes.
    let line: String = "é".repeat(1000);
    let snippet = make_snippet(&line, line.len() / 2, 50);

    // Result must still be valid UTF-8 and contain ~50 é characters.
    assert!(snippet.starts_with('\u{2026}'));
    assert!(snippet.ends_with('\u{2026}'));
    let inner = snippet.trim_matches('\u{2026}');
    assert!(inner.chars().all(|c| c == 'é'));
}
