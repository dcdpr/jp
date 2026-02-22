use std::fmt::Write as _;

use super::*;

struct TestCase<'a> {
    in_out: Vec<(&'a str, Vec<Event>)>,
    flushed: Option<&'a str>,
}

impl TestCase<'_> {
    fn run(self, name: &str) {
        let mut buf = Buffer::new();

        for (input, expected) in self.in_out {
            buf.push(input);
            let actual: Vec<Event> = buf.by_ref().collect();
            assert_eq!(actual, expected, "failed case: {name}");
        }

        assert_eq!(buf.flush().as_deref(), self.flushed, "failed case: {name}");
    }
}

#[test]
fn test_buffer_indented_code() {
    let cases = vec![
        ("simple", TestCase {
            in_out: vec![
                ("    code\n    more\n", vec![]),
                ("Paragraph\n\n", vec![
                    Event::Block("    code\n    more\n".into()),
                    Event::Block("Paragraph\n\n".into()),
                ]),
            ],
            flushed: None,
        }),
        ("with_blank_inside", TestCase {
            in_out: vec![
                ("    foo\n\n", vec![]),
                ("    bar\nText\n\n", vec![
                    Event::Block("    foo\n\n    bar\n".into()),
                    Event::Block("Text\n\n".into()),
                ]),
            ],
            flushed: None,
        }),
        ("ends_on_blank", TestCase {
            in_out: vec![
                ("    foo\n\n", vec![]),
                ("Next\n", vec![Event::Block("    foo\n".into())]),
            ],
            flushed: Some("Next\n"),
        }),
        ("fragmented", TestCase {
            in_out: vec![
                ("    foo", vec![]),
                ("\n    bar\n\nbaz", vec![Event::Block(
                    "    foo\n    bar\n".into(),
                )]),
            ],
            flushed: Some("baz"),
        }),
        ("empty lines within code", TestCase {
            in_out: vec![
                ("    foo", vec![]),
                ("\n    bar\n\n", vec![]),
                ("\n    baz", vec![]),
                ("\nqux", vec![Event::Block(
                    "    foo\n    bar\n\n\n    baz\n".into(),
                )]),
            ],
            flushed: Some("qux"),
        }),
    ];

    for (name, case) in cases {
        case.run(name);
    }
}

#[test]
fn test_buffer_paragraph() {
    let cases = vec![
        ("simple", TestCase {
            in_out: vec![("Paragraph.\n\n", vec![Event::Block(
                "Paragraph.\n\n".into(),
            )])],
            flushed: None,
        }),
        ("no final newline", TestCase {
            in_out: vec![("Paragraph.", vec![])],
            flushed: Some("Paragraph."),
        }),
        ("interrupted by header", TestCase {
            in_out: vec![
                ("Paragraph.\n", vec![]),
                ("# New Header\n", vec![
                    Event::Block("Paragraph.\n".into()),
                    Event::Block("# New Header\n".into()),
                ]),
            ],
            flushed: None,
        }),
        ("interrupted by thematic break", TestCase {
            in_out: vec![
                ("Paragraph.\n\n", vec![Event::Block(
                    "Paragraph.\n\n".into(),
                )]),
                ("---\nAfter\n\n", vec![
                    Event::Block("---\n".into()),
                    Event::Block("After\n\n".into()),
                ]),
            ],
            flushed: None,
        }),
    ];

    for (name, case) in cases {
        case.run(name);
    }
}

#[test]
fn test_buffer_setext_header() {
    let cases = vec![
        ("simple", TestCase {
            in_out: vec![("Header\n===\n", vec![Event::Block("Header\n===\n".into())])],
            flushed: None,
        }),
        ("fragmented", TestCase {
            in_out: vec![
                ("Header\n", vec![]),
                ("===\nNext\n\n", vec![
                    Event::Block("Header\n===\n".into()),
                    Event::Block("Next\n\n".into()),
                ]),
            ],
            flushed: None,
        }),
        ("partial underline", TestCase {
            in_out: vec![
                ("Header\n--", vec![]),
                ("-\n", vec![Event::Block("Header\n---\n".into())]),
            ],
            flushed: None,
        }),
    ];

    for (name, case) in cases {
        case.run(name);
    }
}

#[test]
fn test_buffer_fenced_code_streaming() {
    let cases = vec![
        ("line by line", TestCase {
            in_out: vec![
                ("```rust\n", vec![Event::FencedCodeStart {
                    language: "rust".into(),
                    fence_type: FenceType::Backtick,
                    fence_length: 3,
                }]),
                ("fn main() {\n", vec![Event::FencedCodeLine(
                    "fn main() {\n".into(),
                )]),
                ("}\n", vec![Event::FencedCodeLine("}\n".into())]),
                ("```\n", vec![Event::FencedCodeEnd("```".into())]),
                ("After\n\n", vec![Event::Block("After\n\n".into())]),
            ],
            flushed: None,
        }),
        ("indented fence strips leading spaces", TestCase {
            in_out: vec![
                ("  ```rust\n", vec![Event::FencedCodeStart {
                    language: "rust".into(),
                    fence_type: FenceType::Backtick,
                    fence_length: 3,
                }]),
                ("  fn main() {\n", vec![Event::FencedCodeLine(
                    "fn main() {\n".into(),
                )]),
                ("  ```\n", vec![Event::FencedCodeEnd("```".into())]),
            ],
            flushed: None,
        }),
        ("fragmented across chunks", TestCase {
            in_out: vec![
                ("```rust\nfn main() {", vec![Event::FencedCodeStart {
                    language: "rust".into(),
                    fence_type: FenceType::Backtick,
                    fence_length: 3,
                }]),
                ("}\n```\nAfter\n\n", vec![
                    Event::FencedCodeLine("fn main() {}\n".into()),
                    Event::FencedCodeEnd("```".into()),
                    Event::Block("After\n\n".into()),
                ]),
            ],
            flushed: None,
        }),
        ("longer closing fence", TestCase {
            in_out: vec![("````\ncode\n``````\n", vec![
                Event::FencedCodeStart {
                    language: String::new(),
                    fence_type: FenceType::Backtick,
                    fence_length: 4,
                },
                Event::FencedCodeLine("code\n".into()),
                Event::FencedCodeEnd("````".into()),
            ])],
            flushed: None,
        }),
        ("with blank lines inside", TestCase {
            in_out: vec![("~~~\nHello\n\nWorld\n~~~\n", vec![
                Event::FencedCodeStart {
                    language: String::new(),
                    fence_type: FenceType::Tilde,
                    fence_length: 3,
                },
                Event::FencedCodeLine("Hello\n".into()),
                Event::FencedCodeLine("\n".into()),
                Event::FencedCodeLine("World\n".into()),
                Event::FencedCodeEnd("~~~".into()),
            ])],
            flushed: None,
        }),
    ];

    for (name, case) in cases {
        case.run(name);
    }
}

#[test]
fn test_buffer_thematic_break() {
    let cases = vec![
        ("simple", TestCase {
            in_out: vec![("---\n", vec![Event::Block("---\n".into())])],
            flushed: None,
        }),
        ("with spaces", TestCase {
            in_out: vec![(" * * * \n", vec![Event::Block(" * * * \n".into())])],
            flushed: None,
        }),
    ];

    for (name, case) in cases {
        case.run(name);
    }
}

#[test]
fn test_buffer_link_ref_def() {
    let cases = vec![("simple", TestCase {
        in_out: vec![("[my-link]: https://example.com\n", vec![Event::Block(
            "[my-link]: https://example.com\n".into(),
        )])],
        flushed: None,
    })];

    for (name, case) in cases {
        case.run(name);
    }
}

#[test]
fn test_buffer_html_blocks() {
    let cases = vec![
        ("Type 1 (Script) with blanks", TestCase {
            in_out: vec![
                ("<script>\nvar x = 1;\n\n", vec![]),
                ("console.log(x);\n</script>\n", vec![Event::Block(
                    "<script>\nvar x = 1;\n\nconsole.log(x);\n</script>\n".into(),
                )]),
            ],
            flushed: None,
        }),
        ("Type 6 (Div) fragmented", TestCase {
            in_out: vec![
                ("<div>\n  <p>Hello</p>\n", vec![]),
                ("\nThis is after.", vec![Event::Block(
                    "<div>\n  <p>Hello</p>\n\n".into(),
                )]),
            ],
            flushed: Some("This is after."),
        }),
        ("Type 7 (No Interrupt)", TestCase {
            in_out: vec![
                ("This is a paragraph.\n", vec![]),
                ("<a>foo</a>\n\n", vec![Event::Block(
                    "This is a paragraph.\n<a>foo</a>\n\n".into(),
                )]),
            ],
            flushed: None,
        }),
        ("Type 2 (Comment)", TestCase {
            in_out: vec![
                ("<!-- Hello\n\n", vec![]),
                ("World -->\nPara\n\n", vec![
                    Event::Block("<!-- Hello\n\nWorld -->\n".into()),
                    Event::Block("Para\n\n".into()),
                ]),
            ],
            flushed: None,
        }),
        ("Type 4 (Doctype)", TestCase {
            in_out: vec![("<!DOCTYPE html>\n<p>Hi</p>\n\n", vec![
                Event::Block("<!DOCTYPE html>\n".into()),
                Event::Block("<p>Hi</p>\n\n".into()),
            ])],
            flushed: None,
        }),
    ];

    for (name, case) in cases {
        case.run(name);
    }
}

#[test]
fn test_buffer_misc() {
    let cases = vec![
        ("multiple blocks, one chunk", TestCase {
            in_out: vec![("# Header 1\n\nParagraph.\n\n---\n", vec![
                Event::Block("# Header 1\n".into()),
                Event::Block("Paragraph.\n\n".into()),
                Event::Block("---\n".into()),
            ])],
            flushed: None,
        }),
        ("empty input", TestCase {
            in_out: vec![],
            flushed: None,
        }),
        ("blank lines", TestCase {
            in_out: vec![
                ("\n\n\n", vec![]),
                ("# Header\n", vec![Event::Block("# Header\n".into())]),
                ("\n\nPara\n\n", vec![Event::Block("Para\n\n".into())]),
            ],
            flushed: None,
        }),
    ];

    for (name, case) in cases {
        case.run(name);
    }
}

#[test]
fn test_fmt_write() {
    let mut buf = Buffer::new();
    let _ = writeln!(buf, "# Hello");
    let _ = writeln!(buf, "This is a paragraph.");
    let _ = writeln!(buf, "It has two lines.");

    let actual: Vec<Event> = buf.by_ref().collect();
    assert_eq!(actual, vec![Event::Block("# Hello\n".into())]);

    let _ = writeln!(buf, "\nAnd a new one.");

    let actual: Vec<Event> = buf.by_ref().collect();
    assert_eq!(actual, vec![Event::Block(
        "This is a paragraph.\nIt has two lines.\n\n".into(),
    )]);

    assert_eq!(buf.flush(), Some("And a new one.\n".into()));
}

#[test]
fn test_get_indent() {
    struct Case {
        line: &'static str,
        indent: usize,
        content: &'static str,
    }

    let cases = vec![
        Case {
            line: "    Hello",
            indent: 4,
            content: "Hello",
        },
        Case {
            line: "  Hello",
            indent: 2,
            content: "Hello",
        },
        Case {
            line: "Hello",
            indent: 0,
            content: "Hello",
        },
        Case {
            line: "\tHello",
            indent: 4,
            content: "Hello",
        },
        Case {
            line: " \tHello",
            indent: 4,
            content: "Hello",
        },
        Case {
            line: "  \tHello",
            indent: 4,
            content: "Hello",
        },
        Case {
            line: "   \tHello",
            indent: 4,
            content: "Hello",
        },
        Case {
            line: "    \tHello",
            indent: 8,
            content: "Hello",
        },
        Case {
            line: " \t Hello",
            indent: 5,
            content: "Hello",
        },
    ];

    for case in cases {
        let (indent, content) = get_indent(case.line);
        assert_eq!(indent, case.indent, "indent for {:?}", case.line);
        assert_eq!(content, case.content, "content for {:?}", case.line);
    }
}

#[test]
fn test_is_atx_header() {
    // Valid ATX headers
    assert!(is_atx_header("# Heading"));
    assert!(is_atx_header("## Heading"));
    assert!(is_atx_header("### Heading"));
    assert!(is_atx_header("#### Heading"));
    assert!(is_atx_header("##### Heading"));
    assert!(is_atx_header("###### Heading"));
    assert!(is_atx_header("#\tHeading"));
    assert!(is_atx_header("#"));
    assert!(is_atx_header("##"));
    assert!(is_atx_header("# Heading ##"));

    // Invalid: no space after #
    assert!(!is_atx_header("#5 bolt"));
    assert!(!is_atx_header("#hashtag"));
    assert!(!is_atx_header("#foo"));

    // Invalid: more than 6 #
    assert!(!is_atx_header("####### foo"));
    assert!(!is_atx_header("######## foo"));

    // Invalid: doesn't start with #
    assert!(!is_atx_header("foo # bar"));
    assert!(!is_atx_header(""));
}

#[test]
fn test_buffer_atx_header_validation() {
    // Invalid headers treated as paragraphs
    let invalid = vec![
        ("#hashtag\n\n", vec![Event::Block("#hashtag\n\n".into())]),
        ("#5 bolt\n\n", vec![Event::Block("#5 bolt\n\n".into())]),
        ("####### foo\n\n", vec![Event::Block(
            "####### foo\n\n".into(),
        )]),
    ];

    for (input, expected) in invalid {
        let mut buf = Buffer::new();
        buf.push(input);
        let actual: Vec<Event> = buf.by_ref().collect();
        assert_eq!(actual, expected, "Failed for input: {input:?}");
    }

    // Valid headers
    let valid = vec![
        ("# Valid\n", vec![Event::Block("# Valid\n".into())]),
        ("###### Six\n", vec![Event::Block("###### Six\n".into())]),
        ("#\tTab\n", vec![Event::Block("#\tTab\n".into())]),
        ("#\n", vec![Event::Block("#\n".into())]),
    ];

    for (input, expected) in valid {
        let mut buf = Buffer::new();
        buf.push(input);
        let actual: Vec<Event> = buf.by_ref().collect();
        assert_eq!(actual, expected, "Failed for input: {input:?}");
    }
}

#[test]
fn test_tabs_in_block_detection() {
    // Tab at column 0 = 4 spaces → indented code, not header
    let mut buf = Buffer::new();
    buf.push("\t# Not Header\n\n");
    let actual: Vec<Event> = buf.by_ref().collect();
    assert_eq!(actual, Vec::<Event>::new());
    assert_eq!(buf.flush(), Some("\t# Not Header\n\n".into()));

    // 3 spaces before # = valid header
    let mut buf = Buffer::new();
    buf.push("   # Valid\n");
    let actual: Vec<Event> = buf.by_ref().collect();
    assert_eq!(actual, vec![Event::Block("   # Valid\n".into())]);

    // Tab after # = valid header
    let mut buf = Buffer::new();
    buf.push("#\tFoo\n");
    let actual: Vec<Event> = buf.by_ref().collect();
    assert_eq!(actual, vec![Event::Block("#\tFoo\n".into())]);

    // Tab at column 0 before thematic break = indented code
    let mut buf = Buffer::new();
    buf.push("\t***\n\n");
    let actual: Vec<Event> = buf.by_ref().collect();
    assert_eq!(actual, Vec::<Event>::new());
    assert_eq!(buf.flush(), Some("\t***\n\n".into()));

    // 3 spaces before *** = valid thematic break
    let mut buf = Buffer::new();
    buf.push("   ***\n");
    let actual: Vec<Event> = buf.by_ref().collect();
    assert_eq!(actual, vec![Event::Block("   ***\n".into())]);

    // Mixed tabs and spaces in thematic break
    let mut buf = Buffer::new();
    buf.push("*\t*\t*\t\n");
    let actual: Vec<Event> = buf.by_ref().collect();
    assert_eq!(actual, vec![Event::Block("*\t*\t*\t\n".into())]);
}

#[test]
fn test_buffer_event_display() {
    let cases = vec![
        (Event::Block("Hello".into()), "Hello"),
        (
            Event::FencedCodeStart {
                language: "rust".into(),
                fence_type: FenceType::Backtick,
                fence_length: 3,
            },
            "```rust",
        ),
        (
            Event::FencedCodeStart {
                language: "python".into(),
                fence_type: FenceType::Tilde,
                fence_length: 4,
            },
            "~~~~python",
        ),
        (Event::FencedCodeLine("Hello".into()), "Hello"),
        (Event::FencedCodeEnd("```".into()), "```"),
    ];

    for (event, expected) in cases {
        assert_eq!(event.to_string(), expected);
    }
}
