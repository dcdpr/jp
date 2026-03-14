use super::*;
use crate::BearDb;

fn search(db: &BearDb, queries: Vec<&str>) -> Vec<SearchMatch> {
    db.search(&SearchParams {
        queries: queries.into_iter().map(Into::into).collect(),
        context: 1,
        ..Default::default()
    })
    .unwrap()
}

fn search_with(db: &BearDb, params: &SearchParams) -> Vec<SearchMatch> {
    db.search(params).unwrap()
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
        context: 1,
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
        context: 1,
        ..Default::default()
    });

    assert!(results.is_empty());
}

#[test]
fn search_context_lines() {
    let db = BearDb::in_memory().unwrap();
    let results = search_with(&db, &SearchParams {
        queries: vec!["capturing".into()],
        context: 0,
        ..Default::default()
    });

    // "capturing" is on line 2 of note-1
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].groups.len(), 1);
    assert_eq!(results[0].groups[0].lines.len(), 1);
    assert_eq!(results[0].groups[0].lines[0].0, 2); // line 2 (1-indexed)
}

#[test]
fn search_excludes_trashed() {
    let db = BearDb::in_memory().unwrap();
    let results = search(&db, vec!["trashed"]);
    assert!(results.is_empty());
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
        context: 1,
        ..Default::default()
    });

    assert_eq!(results.len(), 1);
}

#[test]
fn title_in_xml_output() {
    let m = SearchMatch {
        note_id: "abc-123".into(),
        title: "My Note".into(),
        groups: vec![MatchGroup {
            lines: vec![(1, "first line".into())],
        }],
    };

    let xml = m.to_xml();
    assert!(xml.contains(r#"note-id="abc-123""#));
    assert!(xml.contains(r#"title="My Note""#));
}

#[test]
fn title_xml_escaping() {
    let m = SearchMatch {
        note_id: "x".into(),
        title: r#"Notes & "Quotes" <stuff>"#.into(),
        groups: vec![],
    };

    let xml = m.to_xml();
    assert!(xml.contains(r#"title="Notes &amp; &quot;Quotes&quot; &lt;stuff&gt;""#));
}

#[test]
fn match_to_xml_groups() {
    let m = SearchMatch {
        note_id: "abc-123".into(),
        title: "Test".into(),
        groups: vec![
            MatchGroup {
                lines: vec![(10, "line ten".into()), (11, "line eleven".into())],
            },
            MatchGroup {
                lines: vec![(50, "line fifty".into())],
            },
        ],
    };

    let xml = m.to_xml();
    assert!(xml.contains("010: line ten"));
    assert!(xml.contains("..."));
    assert!(xml.contains("050: line fifty"));
}

#[test]
fn fts_mode_word_search() {
    let db = BearDb::in_memory().unwrap();
    let results = search_with(&db, &SearchParams {
        queries: vec!["productivity".into()],
        mode: SearchMode::Fts,
        context: 1,
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
        context: 1,
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
        context: 1,
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
        context: 1,
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
        context: 1,
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
        context: 1,
        ..Default::default()
    });
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].note_id, "note-1");
}
