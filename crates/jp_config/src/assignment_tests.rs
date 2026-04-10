use assert_matches::assert_matches;
use serde_json::json;

use super::*;
use crate::types::json_value::JsonValue;

#[test]
fn test_kv_assignment_from_str() {
    let cases = vec![
        ("foo=bar", KvAssignment {
            key: KvKey {
                path: "foo".to_owned(),
                delim: KeyDelim::Dot,
                full_path: "foo".to_owned(),
            },
            value: KvValue::String("bar".to_owned()),
            strategy: Strategy::Set,
        }),
        ("foo+=bar", KvAssignment {
            key: KvKey {
                path: "foo".to_owned(),
                delim: KeyDelim::Dot,
                full_path: "foo".to_owned(),
            },
            value: KvValue::String("bar".to_owned()),
            strategy: Strategy::Merge,
        }),
        ("foo.bar=baz", KvAssignment {
            key: KvKey {
                path: "foo.bar".to_owned(),
                delim: KeyDelim::Dot,
                full_path: "foo.bar".to_owned(),
            },
            value: KvValue::String("baz".to_owned()),
            strategy: Strategy::Set,
        }),
        ("foo.bar+=baz", KvAssignment {
            key: KvKey {
                path: "foo.bar".to_owned(),
                delim: KeyDelim::Dot,
                full_path: "foo.bar".to_owned(),
            },
            value: KvValue::String("baz".to_owned()),
            strategy: Strategy::Merge,
        }),
        ("foo.bar.1=qux", KvAssignment {
            key: KvKey {
                path: "foo.bar.1".to_owned(),
                delim: KeyDelim::Dot,
                full_path: "foo.bar.1".to_owned(),
            },
            value: KvValue::String("qux".to_owned()),
            strategy: Strategy::Set,
        }),
        ("foo.bar.1+=qux", KvAssignment {
            key: KvKey {
                path: "foo.bar.1".to_owned(),
                delim: KeyDelim::Dot,
                full_path: "foo.bar.1".to_owned(),
            },
            value: KvValue::String("qux".to_owned()),
            strategy: Strategy::Merge,
        }),
        ("foo:=true", KvAssignment {
            key: KvKey {
                path: "foo".to_owned(),
                delim: KeyDelim::Dot,
                full_path: "foo".to_owned(),
            },
            value: KvValue::Json(true.into()),
            strategy: Strategy::Set,
        }),
        ("foo:=42", KvAssignment {
            key: KvKey {
                path: "foo".to_owned(),
                delim: KeyDelim::Dot,
                full_path: "foo".to_owned(),
            },
            value: KvValue::Json(42.into()),
            strategy: Strategy::Set,
        }),
        (r#"foo+:=["bar"]"#, KvAssignment {
            key: KvKey {
                path: "foo".to_owned(),
                delim: KeyDelim::Dot,
                full_path: "foo".to_owned(),
            },
            value: KvValue::Json(vec!["bar".to_owned()].into()),
            strategy: Strategy::Merge,
        }),
    ];

    for (s, expected) in cases {
        let actual = KvAssignment::from_str(s).unwrap();
        assert_eq!(actual, expected);
    }
}

#[test]
fn test_kv_key_trim_prefix() {
    let mut key = KvKey {
        delim: KeyDelim::Dot,
        path: String::new(),
        full_path: String::new(),
    };

    key.delim = KeyDelim::Dot;
    key.path = "foobar.baz".to_owned();
    assert!(key.trim_prefix("foobar"));
    assert_eq!(key.path, "baz");

    key.path = "foobar.baz".to_owned();
    assert!(!key.trim_prefix("foo"));
    assert_eq!(key.path, "foobar.baz");

    key.path = "foobar".to_owned();
    assert!(key.trim_prefix("foobar"));
    assert_eq!(key.path, "");
    //
    key.path = "foobar".to_owned();
    assert!(!key.trim_prefix("foo"));
    assert_eq!(key.path, "foobar");
}

#[test]
fn test_kv_assignment_try_from_cli_env() {
    let cases = vec![
        ("foo", "", "bar", KvAssignment {
            key: KvKey {
                path: "foo".to_owned(),
                delim: KeyDelim::Dot,
                full_path: "foo".to_owned(),
            },
            value: KvValue::String("bar".to_owned()),
            strategy: Strategy::Set,
        }),
        ("foo", "+", "bar", KvAssignment {
            key: KvKey {
                path: "foo".to_owned(),
                delim: KeyDelim::Dot,
                full_path: "foo".to_owned(),
            },
            value: KvValue::String("bar".to_owned()),
            strategy: Strategy::Merge,
        }),
        ("foo.bar", "", "baz", KvAssignment {
            key: KvKey {
                path: "foo.bar".to_owned(),
                delim: KeyDelim::Dot,
                full_path: "foo.bar".to_owned(),
            },
            value: KvValue::String("baz".to_owned()),
            strategy: Strategy::Set,
        }),
        ("foo.bar", "+", "baz", KvAssignment {
            key: KvKey {
                path: "foo.bar".to_owned(),
                delim: KeyDelim::Dot,
                full_path: "foo.bar".to_owned(),
            },
            value: KvValue::String("baz".to_owned()),
            strategy: Strategy::Merge,
        }),
        ("foo.bar.1", "", "qux", KvAssignment {
            key: KvKey {
                path: "foo.bar.1".to_owned(),
                delim: KeyDelim::Dot,
                full_path: "foo.bar.1".to_owned(),
            },
            value: KvValue::String("qux".to_owned()),
            strategy: Strategy::Set,
        }),
        ("foo.bar.1", "+", "qux", KvAssignment {
            key: KvKey {
                path: "foo.bar.1".to_owned(),
                delim: KeyDelim::Dot,
                full_path: "foo.bar.1".to_owned(),
            },
            value: KvValue::String("qux".to_owned()),
            strategy: Strategy::Merge,
        }),
        ("foo", ":", r#""quux""#, KvAssignment {
            key: KvKey {
                path: "foo".to_owned(),
                delim: KeyDelim::Dot,
                full_path: "foo".to_owned(),
            },
            value: KvValue::Json("quux".into()),
            strategy: Strategy::Set,
        }),
        ("foo", "+:", r#"["quux"]"#, KvAssignment {
            key: KvKey {
                path: "foo".to_owned(),
                delim: KeyDelim::Dot,
                full_path: "foo".to_owned(),
            },
            value: KvValue::Json(vec!["quux".to_owned()].into()),
            strategy: Strategy::Merge,
        }),
    ];

    for (k, mods, v, mut expected) in cases {
        for delim in [KeyDelim::Dot, KeyDelim::Underscore] {
            expected.key.delim = delim;
            let actual = match delim {
                KeyDelim::Dot => KvAssignment::try_from_cli(format!("{k}{mods}"), v),
                KeyDelim::Underscore => KvAssignment::try_from_env(k, &format!("{mods}{v}")),
            };

            assert_eq!(actual.unwrap(), expected);
        }
    }
}

#[test]
fn test_kv_assignment_try_from_cli_env_escaped_chars() {
    let cases = vec![
        ("foo=+bar", KvAssignment {
            key: KvKey {
                path: "foo".to_owned(),
                delim: KeyDelim::Underscore,
                full_path: "foo".to_owned(),
            },
            value: KvValue::String("bar".to_owned()),
            strategy: Strategy::Merge,
        }),
        ("foo=\\+bar", KvAssignment {
            key: KvKey {
                path: "foo".to_owned(),
                delim: KeyDelim::Underscore,
                full_path: "foo".to_owned(),
            },
            value: KvValue::String("+bar".to_owned()),
            strategy: Strategy::Set,
        }),
        ("foo=\\:bar", KvAssignment {
            key: KvKey {
                path: "foo".to_owned(),
                delim: KeyDelim::Underscore,
                full_path: "foo".to_owned(),
            },
            value: KvValue::String(":bar".to_owned()),
            strategy: Strategy::Set,
        }),
        ("foo=:true", KvAssignment {
            key: KvKey {
                path: "foo".to_owned(),
                delim: KeyDelim::Underscore,
                full_path: "foo".to_owned(),
            },
            value: KvValue::Json(true.into()),
            strategy: Strategy::Set,
        }),
    ];

    for (s, expected) in cases {
        let (k, v) = s.split_once('=').unwrap();
        let actual = KvAssignment::try_from_env(k, v).unwrap();
        assert_eq!(actual, expected);
    }
}

#[test]
fn test_kv_assignment_try_object() {
    #[derive(Debug, PartialEq, serde::Deserialize)]
    struct Test {
        foo: String,
    }

    let kv = KvAssignment::try_from_cli(":", r#"{"foo":"bar"}"#).unwrap();
    assert_eq!(kv.try_object::<Test>().unwrap(), Test { foo: "bar".into() });

    let kv = KvAssignment::try_from_cli("foo", r#""bar""#).unwrap();
    assert_matches!(
        kv.try_object::<Test>().unwrap_err().error,
        KvAssignmentErrorKind::Type { need, .. } if need == ["object"]
    );
}

#[test]
fn test_kv_assignment_try_object_or_from_str() {
    #[derive(Debug, PartialEq, serde::Deserialize)]
    struct Test {
        foo: String,
    }

    impl FromStr for Test {
        type Err = BoxedError;

        fn from_str(s: &str) -> Result<Self, Self::Err> {
            Ok(Self { foo: s.to_owned() })
        }
    }

    let kv = KvAssignment::try_from_cli(":", r#"{"foo":"bar"}"#).unwrap();
    assert_eq!(kv.try_object_or_from_str::<Test, _>().unwrap(), Test {
        foo: "bar".into()
    });

    let kv = KvAssignment::try_from_cli("foo:", r#""bar""#).unwrap();
    assert_eq!(kv.try_object_or_from_str::<Test, _>().unwrap(), Test {
        foo: "bar".into()
    });

    let kv = KvAssignment::try_from_cli("foo", "bar").unwrap();
    assert_eq!(kv.try_object_or_from_str::<Test, _>().unwrap(), Test {
        foo: "bar".into()
    });

    let kv = KvAssignment::try_from_cli("foo:", "42").unwrap();
    assert_matches!(
        kv.try_object_or_from_str::<Test, _>().unwrap_err().error,
        KvAssignmentErrorKind::Type { need, .. } if need == ["object", "string"]
    );
}

#[test]
fn test_kv_assignment_try_string() {
    let kv = KvAssignment::try_from_cli("", "bar").unwrap();
    assert_eq!(kv.try_string().unwrap(), "bar");

    let kv = KvAssignment::try_from_cli(":", r#""bar""#).unwrap();
    assert_eq!(kv.try_string().unwrap(), "bar");

    let kv = KvAssignment::try_from_cli(":", "null").unwrap();
    assert_matches!(
        kv.try_string().unwrap_err().error,
        KvAssignmentErrorKind::Type { need, .. } if need == ["string"]
    );
}

#[test]
fn test_kv_assignment_try_bool() {
    let kv = KvAssignment::try_from_cli("", "true").unwrap();
    assert!(kv.try_bool().unwrap());

    let kv = KvAssignment::try_from_cli(":", "true").unwrap();
    assert!(kv.try_bool().unwrap());

    let kv = KvAssignment::try_from_cli("", "false").unwrap();
    assert!(!kv.try_bool().unwrap());

    let kv = KvAssignment::try_from_cli(":", "false").unwrap();
    assert!(!kv.try_bool().unwrap());

    let kv = KvAssignment::try_from_cli("", "bar").unwrap();
    assert_matches!(
        kv.try_bool().unwrap_err().error,
        KvAssignmentErrorKind::ParseBool { .. }
    );

    let kv = KvAssignment::try_from_cli(":", r#"{"foo":"bar"}"#).unwrap();
    assert_matches!(
        kv.try_bool().unwrap_err().error,
        KvAssignmentErrorKind::Type { need, .. } if need == ["bool", "string"]
    );
}

#[test]
fn test_kv_assignment_try_bool_or_from_str() {
    // A type that accepts both bools and a string variant.
    #[derive(Debug, PartialEq)]
    enum Mode {
        Yes,
        No,
        Maybe,
    }

    impl From<bool> for Mode {
        fn from(v: bool) -> Self {
            if v { Self::Yes } else { Self::No }
        }
    }

    impl FromStr for Mode {
        type Err = String;

        fn from_str(s: &str) -> Result<Self, Self::Err> {
            match s {
                "true" | "yes" => Ok(Self::Yes),
                "false" | "no" => Ok(Self::No),
                "maybe" => Ok(Self::Maybe),
                _ => Err(format!("invalid: {s}")),
            }
        }
    }

    // JSON bool → From<bool>
    let kv = KvAssignment::try_from_cli(":", "true").unwrap();
    assert_eq!(kv.try_bool_or_from_str::<Mode, _>().unwrap(), Mode::Yes);

    let kv = KvAssignment::try_from_cli(":", "false").unwrap();
    assert_eq!(kv.try_bool_or_from_str::<Mode, _>().unwrap(), Mode::No);

    // CLI string → FromStr
    let kv = KvAssignment::try_from_cli("", "maybe").unwrap();
    assert_eq!(kv.try_bool_or_from_str::<Mode, _>().unwrap(), Mode::Maybe);

    let kv = KvAssignment::try_from_cli("", "true").unwrap();
    assert_eq!(kv.try_bool_or_from_str::<Mode, _>().unwrap(), Mode::Yes);

    // JSON string → FromStr
    let kv = KvAssignment::try_from_cli(":", r#""maybe""#).unwrap();
    assert_eq!(kv.try_bool_or_from_str::<Mode, _>().unwrap(), Mode::Maybe);

    // Invalid JSON type
    let kv = KvAssignment::try_from_cli(":", "42").unwrap();
    assert_matches!(
        kv.try_bool_or_from_str::<Mode, _>().unwrap_err().error,
        KvAssignmentErrorKind::Type { need, .. } if need == ["bool", "string"]
    );

    // Invalid string value
    let kv = KvAssignment::try_from_cli("", "nope").unwrap();
    assert_matches!(
        kv.try_bool_or_from_str::<Mode, _>().unwrap_err().error,
        KvAssignmentErrorKind::Parse { .. }
    );
}

#[test]
fn test_kv_assignment_try_u32() {
    let kv = KvAssignment::try_from_cli("foo", "42").unwrap();
    assert_eq!(kv.try_u32().unwrap(), 42);

    let kv = KvAssignment::try_from_cli("foo:", "42").unwrap();

    assert_eq!(kv.try_u32().unwrap(), 42);

    let kv = KvAssignment::try_from_cli(":", "true").unwrap();
    assert_matches!(
        kv.try_u32().unwrap_err().error,
        KvAssignmentErrorKind::Type { need, .. } if need == ["number", "string"]
    );

    let kv = KvAssignment::try_from_cli("", "bar").unwrap();
    assert_matches!(
        kv.try_u32().unwrap_err().error,
        KvAssignmentErrorKind::ParseInt { .. }
    );
}

#[test]
#[expect(clippy::too_many_lines)]
fn test_kv_assignment_try_vec_of_nested() {
    #[derive(Debug, schematic::Config)]
    #[expect(dead_code)]
    struct Test {
        one: String,
        two: String,
    }

    impl FromStr for PartialTest {
        type Err = BoxedError;

        fn from_str(s: &str) -> Result<Self, Self::Err> {
            if s == "whoops" {
                return Err(Box::<dyn std::error::Error + Send + Sync>::from("whoops"));
            }

            Ok(Self {
                one: Some(s.to_owned()),
                two: None,
            })
        }
    }

    impl AssignKeyValue for PartialTest {
        fn assign(&mut self, kv: KvAssignment) -> Result<(), BoxedError> {
            match kv.key_string().as_str() {
                "one" => self.one = Some(kv.try_string()?),
                "two" => self.two = Some(kv.try_string()?),
                _ => return missing_key(&kv),
            }

            Ok(())
        }
    }

    let mut v = vec![PartialTest::default()];
    let kv = KvAssignment::try_from_cli("0.one", "bar").unwrap();
    kv.try_vec_of_nested(&mut v).unwrap();
    assert_eq!(v[0].one, Some("bar".into()));

    let mut v = vec![PartialTest::default()];
    let kv = KvAssignment::try_from_cli(":", r#"[{ "one": "1" }, { "two": "2" }]"#).unwrap();
    kv.try_vec_of_nested(&mut v).unwrap();
    assert_eq!(v, vec![
        PartialTest {
            one: Some("1".into()),
            two: None
        },
        PartialTest {
            one: None,
            two: Some("2".into()),
        }
    ]);

    let mut v = vec![PartialTest::default()];
    let kv = KvAssignment::try_from_cli("0:", r#"{ "one": "qux" }"#).unwrap();
    kv.try_vec_of_nested(&mut v).unwrap();
    assert_eq!(v[0].one, Some("qux".into()));

    let mut v = vec![PartialTest::default()];
    let kv = KvAssignment::try_from_cli("0", "quux").unwrap();
    kv.try_vec_of_nested(&mut v).unwrap();
    assert_eq!(v[0].one, Some("quux".into()));

    let mut v = vec![PartialTest::default()];
    let kv = KvAssignment::try_from_cli(":", "[]").unwrap();
    kv.try_vec_of_nested(&mut v).unwrap();
    assert!(v.is_empty());

    let mut v = vec![PartialTest {
        one: None,
        two: Some("foo".into()),
    }];
    let kv = KvAssignment::try_from_cli("+", "bar").unwrap();
    kv.try_vec_of_nested(&mut v).unwrap();
    assert_eq!(v, vec![
        PartialTest {
            one: None,
            two: Some("foo".into()),
        },
        PartialTest {
            one: Some("bar".into()),
            two: None
        }
    ]);

    let mut v = vec![PartialTest::default()];
    let kv = KvAssignment::try_from_cli("one", "qux").unwrap();
    let error = kv.try_vec_of_nested(&mut v).unwrap_err();
    assert_eq!(error.to_string(), "one: type error");

    let mut v = vec![PartialTest::default()];
    let kv = KvAssignment::try_from_cli("1.one", "qux").unwrap();
    let error = kv.try_vec_of_nested(&mut v).unwrap_err();
    assert_eq!(error.to_string(), "1.one: unknown index");

    let mut v = vec![PartialTest::default()];
    let kv = KvAssignment::try_from_cli("0.three", "qux").unwrap();
    let error = kv.try_vec_of_nested(&mut v).unwrap_err();
    assert_eq!(error.to_string(), "0.three: unknown key");

    let mut v = vec![PartialTest::default()];
    let kv = KvAssignment::try_from_cli("0:", "true").unwrap();
    let error = kv.try_vec_of_nested(&mut v).unwrap_err();
    assert_eq!(error.to_string(), "0: type error");

    let mut v = vec![PartialTest::default()];
    let kv = KvAssignment::try_from_cli("0.one:", "true").unwrap();
    let error = kv.try_vec_of_nested(&mut v).unwrap_err();
    assert_eq!(error.to_string(), "0.one: type error");

    let mut v = vec![PartialTest::default()];
    let kv = KvAssignment::try_from_cli("0", "whoops").unwrap();
    let error = kv.try_vec_of_nested(&mut v).unwrap_err();
    assert_eq!(error.to_string(), "0: parse error");

    let mut v = vec![PartialTest::default()];
    let kv = KvAssignment::try_from_cli(":", "42").unwrap();
    let error = kv.try_vec_of_nested(&mut v).unwrap_err();
    assert_eq!(error.to_string(), ": type error");
}

#[test]
fn test_kv_assignment_try_vec_of_strings() {
    let mut v = vec!["foo".to_owned()];
    let kv = KvAssignment::try_from_cli("", "bar").unwrap();
    kv.try_vec_of_strings(&mut v).unwrap();
    assert_eq!(v, vec!["bar".to_owned()]);

    let mut v: Vec<String> = vec![];
    let kv = KvAssignment::try_from_cli("", "foo,bar").unwrap();
    kv.try_vec_of_strings(&mut v).unwrap();
    assert_eq!(v, vec!["foo".to_owned(), "bar".to_owned()]);

    let mut v = vec!["foo".to_owned()];
    let kv = KvAssignment::try_from_cli("0", "bar").unwrap();
    kv.try_vec_of_strings(&mut v).unwrap();
    assert_eq!(v, vec!["bar".to_owned()]);

    let mut v = vec!["foo".to_owned()];
    let kv = KvAssignment::try_from_cli("2", "bar").unwrap();
    let error = kv.try_vec_of_strings(&mut v).unwrap_err();
    assert_eq!(&error.to_string(), "2: unknown index");
}

#[test]
fn test_assign_to_entry_sets_value() {
    let mut map = IndexMap::<String, JsonValue>::new();
    let kv = KvAssignment::try_from_cli("port", "3000").unwrap();
    kv.assign_to_entry(&mut map).unwrap();
    assert_eq!(map["port"], JsonValue(json!("3000")));
}

#[test]
fn test_assign_to_entry_nested_key() {
    let mut map = IndexMap::<String, JsonValue>::new();
    let kv = KvAssignment::try_from_cli("web.port", "3000").unwrap();
    kv.assign_to_entry(&mut map).unwrap();
    assert_eq!(map["web"], JsonValue(json!({"port": "3000"})));
}

#[test]
fn test_assign_to_entry_preserves_existing() {
    let mut map = IndexMap::<String, JsonValue>::new();
    map.insert("host".to_owned(), JsonValue(json!("localhost")));

    let kv = KvAssignment::try_from_cli("port", "3000").unwrap();
    kv.assign_to_entry(&mut map).unwrap();

    assert_eq!(map["host"], JsonValue(json!("localhost")));
    assert_eq!(map["port"], JsonValue(json!("3000")));
}

#[test]
fn test_assign_to_entry_merges_into_existing() {
    let mut map = IndexMap::<String, JsonValue>::new();
    map.insert("web".to_owned(), JsonValue(json!({"host": "localhost"})));

    let kv = KvAssignment::try_from_cli("web.port", "3000").unwrap();
    kv.assign_to_entry(&mut map).unwrap();

    assert_eq!(
        map["web"],
        JsonValue(json!({"host": "localhost", "port": "3000"}))
    );
}
