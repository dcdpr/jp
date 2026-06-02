use url::Url;

use super::{Namespace, Reference};

#[test]
fn parses_all_spellings_to_same_reference() {
    let spellings = [
        "ag://issues/592",
        "ag:issues/592",
        "ag://issue/592",
        "ag:issue/592",
        "ag://592",
        "ag:592",
    ];

    for spelling in spellings {
        let reference = Reference::parse(&Url::parse(spelling).unwrap())
            .unwrap_or_else(|e| panic!("`{spelling}` should parse: {e}"));
        assert_eq!(reference.namespace(), Namespace::Issues);
        assert_eq!(reference.id(), "592", "id mismatch for `{spelling}`");
    }
}

#[test]
fn renders_canonical_url() {
    let reference = Reference::parse(&Url::parse("ag:592").unwrap()).unwrap();
    assert_eq!(reference.to_url().unwrap().as_str(), "ag://issues/592");
}

#[test]
fn rejects_unsupported_uris() {
    let cases = [
        "gh://issues/1",      // wrong scheme
        "ag://prs/1",         // unknown namespace
        "ag://issues/abc",    // non-numeric id
        "ag:issues/12/extra", // too many segments
        "ag:",                // empty
    ];

    for case in cases {
        let parsed = Reference::parse(&Url::parse(case).unwrap());
        assert!(parsed.is_err(), "`{case}` should be rejected");
    }
}
