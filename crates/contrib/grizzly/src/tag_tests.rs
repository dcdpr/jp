use crate::BearDb;

#[test]
fn list_tags() {
    let db = BearDb::in_memory().unwrap();
    let tags = db.tags().unwrap();
    assert_eq!(tags.len(), 3);
    assert_eq!(tags[0].name, "personal");
    assert_eq!(tags[1].name, "productivity");
    assert_eq!(tags[2].name, "projects/jp");
}
