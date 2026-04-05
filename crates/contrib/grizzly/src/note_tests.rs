use super::*;
use crate::BearDb;

#[test]
fn get_note_by_id() {
    let db = BearDb::in_memory().unwrap();
    let notes = db.get_notes(&["note-1"]).unwrap();

    assert_eq!(notes.len(), 1);
    assert_eq!(notes[0].title, "Getting Things Done");
    assert_eq!(notes[0].tags, vec!["productivity", "projects/jp"]);
}

#[test]
fn trashed_notes_excluded() {
    let db = BearDb::in_memory().unwrap();
    let notes = db.get_notes(&["note-4"]).unwrap();
    assert!(notes.is_empty());
}

#[test]
fn missing_note_returns_empty() {
    let db = BearDb::in_memory().unwrap();
    let notes = db.get_notes(&["nonexistent"]).unwrap();
    assert!(notes.is_empty());
}

#[test]
fn note_xml_format() {
    let note = Note {
        id: "abc-123".into(),
        title: "Test".into(),
        content: Some("Line 1\nLine 2".into()),
        tags: vec!["tag1".into(), "tag2".into()],
        updated_at: Some("2024-01-01 00:00:00".into()),
    };

    let xml = note.to_xml();
    assert!(xml.contains(r#"id="abc-123""#));
    assert!(xml.contains(r#"tags="tag1 tag2""#));
    assert!(xml.contains("Line 1\nLine 2"));
}

#[test]
fn parse_line_specs() {
    assert_eq!(
        LineSpec::parse(&serde_json::json!(10)),
        Some(LineSpec::Single(10))
    );
    assert_eq!(
        LineSpec::parse(&serde_json::json!("10:20")),
        Some(LineSpec::Range(10, 20))
    );
    assert_eq!(
        LineSpec::parse(&serde_json::json!("10-20")),
        Some(LineSpec::Range(10, 20))
    );
    assert_eq!(
        LineSpec::parse(&serde_json::json!("10..20")),
        Some(LineSpec::Range(10, 20))
    );
    assert_eq!(LineSpec::parse(&serde_json::json!("garbage")), None);
}
