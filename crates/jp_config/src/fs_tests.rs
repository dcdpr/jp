use test_log::test;

use super::*;

#[test]
fn test_expand_tilde() {
    struct TestCase {
        path: &'static str,
        home: Option<&'static str>,
        expected: Option<&'static str>,
    }

    let cases = vec![
        ("no tilde with home", TestCase {
            path: "no/tilde/here",
            home: Some("/tmp"),
            expected: Some("no/tilde/here"),
        }),
        ("no tilde missing home", TestCase {
            path: "no/tilde/here",
            home: None,
            expected: Some("no/tilde/here"),
        }),
        ("tilde path with home", TestCase {
            path: "~/subdir",
            home: Some("/tmp"),
            expected: Some("/tmp/subdir"),
        }),
        ("only tilde with home", TestCase {
            path: "~",
            home: Some("/tmp"),
            expected: Some("/tmp"),
        }),
        ("tilde missing home", TestCase {
            path: "~",
            home: None,
            expected: None,
        }),
    ];

    for (name, case) in cases {
        assert_eq!(
            expand_tilde(case.path, case.home),
            case.expected.map(Utf8PathBuf::from),
            "Failed test case: {name}"
        );
    }
}
