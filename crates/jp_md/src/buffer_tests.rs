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

        assert_partial_flush(&mut buf, self.flushed, name);
    }
}

/// Helper for tests that expected the old `Buffer::flush() -> Option<String>`
/// API.
/// Asserts the buffer's remaining content (via `flush_events`) matches
/// `expected` as a *single* trailing `Flush` event with indent=0, or `None` for
/// an empty buffer.
#[track_caller]
fn assert_partial_flush(buf: &mut Buffer, expected: Option<&str>, name: &str) {
    let events = buf.flush_events();
    match (events.as_slice(), expected) {
        ([], None) => {}
        ([Event::Flush { content, indent: 0 }], Some(exp)) => {
            assert_eq!(content, exp, "failed case ({name}): flush content mismatch");
        }
        (events, exp) => panic!(
            "failed case ({name}): expected single Flush with content={exp:?}, indent=0; got \
             {events:?}"
        ),
    }
}

#[test]
fn test_buffer_indented_code() {
    let cases = vec![
        ("simple", TestCase {
            in_out: vec![
                ("    code\n    more\n", vec![]),
                ("Paragraph\n\n", vec![
                    Event::block("    code\n    more\n"),
                    Event::block("Paragraph\n\n"),
                ]),
            ],
            flushed: None,
        }),
        ("with_blank_inside", TestCase {
            in_out: vec![
                ("    foo\n\n", vec![]),
                ("    bar\nText\n\n", vec![
                    Event::block("    foo\n\n    bar\n"),
                    Event::block("Text\n\n"),
                ]),
            ],
            flushed: None,
        }),
        ("ends_on_blank", TestCase {
            in_out: vec![
                ("    foo\n\n", vec![]),
                ("Next\n", vec![Event::block("    foo\n")]),
            ],
            flushed: Some("Next\n"),
        }),
        ("fragmented", TestCase {
            in_out: vec![
                ("    foo", vec![]),
                ("\n    bar\n\nbaz", vec![Event::block("    foo\n    bar\n")]),
            ],
            flushed: Some("baz"),
        }),
        ("empty lines within code", TestCase {
            in_out: vec![
                ("    foo", vec![]),
                ("\n    bar\n\n", vec![]),
                ("\n    baz", vec![]),
                ("\nqux", vec![Event::block(
                    "    foo\n    bar\n\n\n    baz\n",
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
            in_out: vec![("Paragraph.\n\n", vec![Event::block("Paragraph.\n\n")])],
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
                    Event::block("Paragraph.\n"),
                    Event::block("# New Header\n"),
                ]),
            ],
            flushed: None,
        }),
        ("interrupted by thematic break", TestCase {
            in_out: vec![
                ("Paragraph.\n\n", vec![Event::block("Paragraph.\n\n")]),
                ("---\nAfter\n\n", vec![
                    Event::block("---\n"),
                    Event::block("After\n\n"),
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
            in_out: vec![("Header\n===\n", vec![Event::block("Header\n===\n")])],
            flushed: None,
        }),
        ("fragmented", TestCase {
            in_out: vec![
                ("Header\n", vec![]),
                ("===\nNext\n\n", vec![
                    Event::block("Header\n===\n"),
                    Event::block("Next\n\n"),
                ]),
            ],
            flushed: None,
        }),
        ("partial underline", TestCase {
            in_out: vec![
                ("Header\n--", vec![]),
                ("-\n", vec![Event::block("Header\n---\n")]),
            ],
            flushed: None,
        }),
    ];

    for (name, case) in cases {
        case.run(name);
    }
}

#[test]
#[expect(clippy::too_many_lines)]
fn test_buffer_fenced_code_streaming() {
    let cases = vec![
        ("line by line", TestCase {
            in_out: vec![
                ("```rust\n", vec![Event::FencedCodeStart {
                    language: "rust".into(),
                    fence_type: FenceType::Backtick,
                    fence_length: 3,
                    indent: 0,
                }]),
                ("fn main() {\n", vec![Event::fenced_code_line(
                    "fn main() {\n",
                )]),
                ("}\n", vec![Event::fenced_code_line("}\n")]),
                ("```\n", vec![Event::fenced_code_end("```")]),
                ("After\n\n", vec![Event::block("After\n\n")]),
            ],
            flushed: None,
        }),
        ("indented fence strips leading spaces", TestCase {
            in_out: vec![
                ("  ```rust\n", vec![Event::FencedCodeStart {
                    language: "rust".into(),
                    fence_type: FenceType::Backtick,
                    fence_length: 3,
                    indent: 2,
                }]),
                ("  fn main() {\n", vec![Event::FencedCodeLine {
                    content: "fn main() {\n".into(),
                    indent: 2,
                }]),
                ("  ```\n", vec![Event::FencedCodeEnd {
                    fence: "```".into(),
                    indent: 2,
                }]),
            ],
            flushed: None,
        }),
        ("fragmented across chunks", TestCase {
            in_out: vec![
                ("```rust\nfn main() {", vec![Event::FencedCodeStart {
                    language: "rust".into(),
                    fence_type: FenceType::Backtick,
                    fence_length: 3,
                    indent: 0,
                }]),
                ("}\n```\nAfter\n\n", vec![
                    Event::fenced_code_line("fn main() {}\n"),
                    Event::fenced_code_end("```"),
                    Event::block("After\n\n"),
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
                    indent: 0,
                },
                Event::fenced_code_line("code\n"),
                Event::fenced_code_end("````"),
            ])],
            flushed: None,
        }),
        ("with blank lines inside", TestCase {
            in_out: vec![("~~~\nHello\n\nWorld\n~~~\n", vec![
                Event::FencedCodeStart {
                    language: String::new(),
                    fence_type: FenceType::Tilde,
                    fence_length: 3,
                    indent: 0,
                },
                Event::fenced_code_line("Hello\n"),
                Event::fenced_code_line("\n"),
                Event::fenced_code_line("World\n"),
                Event::fenced_code_end("~~~"),
            ])],
            flushed: None,
        }),
        ("multiple consecutive blank lines preserved", TestCase {
            in_out: vec![("```\n\n\n\n```\n", vec![
                Event::FencedCodeStart {
                    language: String::new(),
                    fence_type: FenceType::Backtick,
                    fence_length: 3,
                    indent: 0,
                },
                Event::fenced_code_line("\n"),
                Event::fenced_code_line("\n"),
                Event::fenced_code_line("\n"),
                Event::fenced_code_end("```"),
            ])],
            flushed: None,
        }),
        ("five consecutive blank lines preserved", TestCase {
            in_out: vec![("```\n\n\n\n\n\n```\n", vec![
                Event::FencedCodeStart {
                    language: String::new(),
                    fence_type: FenceType::Backtick,
                    fence_length: 3,
                    indent: 0,
                },
                Event::fenced_code_line("\n"),
                Event::fenced_code_line("\n"),
                Event::fenced_code_line("\n"),
                Event::fenced_code_line("\n"),
                Event::fenced_code_line("\n"),
                Event::fenced_code_end("```"),
            ])],
            flushed: None,
        }),
        ("blank lines between code lines preserved", TestCase {
            in_out: vec![("```\nfoo\n\n\n\nbar\n```\n", vec![
                Event::FencedCodeStart {
                    language: String::new(),
                    fence_type: FenceType::Backtick,
                    fence_length: 3,
                    indent: 0,
                },
                Event::fenced_code_line("foo\n"),
                Event::fenced_code_line("\n"),
                Event::fenced_code_line("\n"),
                Event::fenced_code_line("\n"),
                Event::fenced_code_line("bar\n"),
                Event::fenced_code_end("```"),
            ])],
            flushed: None,
        }),
    ];

    for (name, case) in cases {
        case.run(name);
    }
}

#[test]
fn test_buffer_nested_fenced_code() {
    // Bug: when LLM produces a markdown code block containing an inner code
    // block with the same backtick count, the inner closing fence prematurely
    // closes the outer block. Everything after gets misinterpreted.
    let input =
        "```markdown\nfoo bar\n\n```rust\nfn main() {}\n```\n\nbaz\n\n```\n\nregular paragraph\n\n";

    let mut buf = Buffer::new();
    buf.push(input);
    let events: Vec<Event> = buf.by_ref().collect();

    assert_eq!(events, vec![
        Event::FencedCodeStart {
            language: "markdown".into(),
            fence_type: FenceType::Backtick,
            fence_length: 3,
            indent: 0,
        },
        Event::fenced_code_line("foo bar\n"),
        Event::fenced_code_line("\n"),
        // Inner fence opening — treated as code content, depth increments.
        Event::fenced_code_line("```rust\n"),
        Event::fenced_code_line("fn main() {}\n"),
        // Inner fence closing — depth decrements, still code content.
        Event::fenced_code_line("```\n"),
        Event::fenced_code_line("\n"),
        Event::fenced_code_line("baz\n"),
        Event::fenced_code_line("\n"),
        // Actual closing fence — depth is 0, closes the outer block.
        Event::fenced_code_end("```"),
        Event::block("regular paragraph\n\n"),
    ]);

    assert_eq!(buf.flush_events(), Vec::<Event>::new());
}

#[test]
fn test_buffer_flush_closes_fence_without_trailing_newline() {
    // A closing fence that is the last line of the stream with no trailing
    // newline must still be recognized as a close, not dumped as text.
    let mut buf = Buffer::new();
    buf.push("```sh\necho hi\n```");

    let streamed: Vec<Event> = buf.by_ref().collect();
    assert_eq!(streamed, vec![
        Event::FencedCodeStart {
            language: "sh".into(),
            fence_type: FenceType::Backtick,
            fence_length: 3,
            indent: 0,
        },
        Event::fenced_code_line("echo hi\n"),
    ]);

    assert_eq!(buf.flush_events(), vec![Event::fenced_code_end("```")]);
}

#[test]
fn test_buffer_flush_synthesizes_close_for_unterminated_fence() {
    // A fenced block that never closes is balanced with a synthetic closing
    // fence at flush, so consumers can match the opening fence.
    let mut buf = Buffer::new();
    buf.push("```rust\nlet x = 1;\n");

    let streamed: Vec<Event> = buf.by_ref().collect();
    assert_eq!(streamed, vec![
        Event::FencedCodeStart {
            language: "rust".into(),
            fence_type: FenceType::Backtick,
            fence_length: 3,
            indent: 0,
        },
        Event::fenced_code_line("let x = 1;\n"),
    ]);

    assert_eq!(buf.flush_events(), vec![Event::fenced_code_end("```")]);
}

#[test]
fn test_buffer_flush_emits_partial_last_code_line_then_close() {
    // The final code line lacks a trailing newline and there is no closing
    // fence: end-of-region completes the line, then a synthetic close
    // balances the block.
    let mut buf = Buffer::new();
    buf.push("```sh\necho hi");

    let streamed: Vec<Event> = buf.by_ref().collect();
    assert_eq!(streamed, vec![Event::FencedCodeStart {
        language: "sh".into(),
        fence_type: FenceType::Backtick,
        fence_length: 3,
        indent: 0,
    }]);

    assert_eq!(buf.flush_events(), vec![
        Event::fenced_code_line("echo hi\n"),
        Event::fenced_code_end("```"),
    ]);
}

#[test]
fn test_buffer_flush_events_renumbers_partial_list_items() {
    // The buffer can't flush items until the *next* line has arrived
    // with a complete newline, so when a stream ends with a partial
    // last line in a list, several items can pile up. `flush_events`
    // splits the remainder at sibling-marker boundaries, emits each
    // complete item as its own `Block` (with renumbering), and emits
    // the final partial line as a `Flush`.
    let input = "5. First\n7. Second\n9. Third without trailing newline";
    let mut buf = Buffer::new();
    buf.push(input);
    let events: Vec<Event> = buf.by_ref().collect();

    // Only the first item flushed normally. "7. Second" can't flush
    // via next() because the line after it ("9. Third...") is
    // incomplete; the buffer waits.
    assert_eq!(events, vec![Event::Block {
        content: "5. First\n".into(),
        indent: 0,
    }]);

    // flush_events splits the remaining two items: "7. Second\n"
    // becomes a complete `Block` (renumbered `7.` -> `6.`); the partial
    // "9. Third without trailing newline" becomes a `Flush` (renumbered
    // `9.` -> `7.`).
    let flushed = buf.flush_events();
    assert_eq!(flushed, vec![
        Event::Block {
            content: "6. Second\n".into(),
            indent: 0,
        },
        Event::Flush {
            content: "7. Third without trailing newline".into(),
            indent: 0,
        },
    ]);
}

#[test]
fn test_buffer_flush_events_resets_state() {
    // After `flush_events`, the buffer should be in `AtBoundary` with
    // no parents stack, so subsequent content is parsed as fresh
    // top-level blocks rather than as continuation of the just-flushed
    // block. `ChatRenderer::flush()` calls `flush_events` on every
    // content-kind transition (reasoning ↔ message ↔ tool call, role
    // headers, user echos), not only at process teardown — so a stale
    // state here corrupts the next chunk's parsing.
    let mut buf = Buffer::new();
    buf.push("1. one\n2. two\n");

    // First item flushes normally via `next()`.
    let events: Vec<Event> = buf.by_ref().collect();
    assert_eq!(events, vec![Event::block("1. one\n")]);

    // flush_events drains the rest.
    let flushed = buf.flush_events();
    assert_eq!(flushed, vec![Event::Flush {
        content: "2. two\n".into(),
        indent: 0,
    }]);

    // A fresh paragraph after the flush must be parsed as a new
    // top-level block, not as continuation of the previous list.
    buf.push("paragraph\n\n");
    let events: Vec<Event> = buf.by_ref().collect();
    assert_eq!(events, vec![Event::block("paragraph\n\n")]);
}

#[test]
fn test_buffer_flush_events_resets_state_empty_buffer_path() {
    // The empty-buffer fast path also needs the reset: a caller may
    // consume the last block, then call `flush_events` again as a
    // "wipe the slate" no-op before pushing fresh content.
    let mut buf = Buffer::new();
    buf.push("- item\n");
    // Buffer enters `InList` but doesn't flush yet (no sibling marker
    // arrived). `next()` consumes nothing.
    let events: Vec<Event> = buf.by_ref().collect();
    assert!(events.is_empty(), "got: {events:?}");

    // Consume the item via `flush_events`.
    let flushed = buf.flush_events();
    assert_eq!(flushed, vec![Event::Flush {
        content: "- item\n".into(),
        indent: 0,
    }]);

    // A second `flush_events` with empty data must still reset.
    assert_eq!(buf.flush_events(), Vec::<Event>::new());

    // Fresh content parses as a paragraph, not as list continuation.
    buf.push("paragraph\n\n");
    let events: Vec<Event> = buf.by_ref().collect();
    assert_eq!(events, vec![Event::block("paragraph\n\n")]);
}

#[test]
fn test_buffer_partial_list_flush_renders_correctly_end_to_end() {
    // End-to-end guarantee for the test above: the rendered terminal
    // output renumbers all items sequentially even though the buffer
    // only renumbers the first marker of the partial flush.
    use crate::format::{Formatter, TerminalOptions};

    let input = "5. First\n7. Second\n9. Third without trailing newline";
    let mut buf = Buffer::new();
    buf.push(input);
    let f = Formatter::with_width(0);
    let mut rendered = String::new();
    for ev in buf.by_ref() {
        if let Event::Block { content, indent } = ev {
            let opts = TerminalOptions {
                indent,
                ..Default::default()
            };
            rendered.push_str(&f.format_terminal_with(&content, &opts).unwrap());
        }
    }
    for ev in buf.flush_events() {
        if let Event::Block { content, indent } | Event::Flush { content, indent } = ev {
            let opts = TerminalOptions {
                indent,
                ..Default::default()
            };
            rendered.push_str(&f.format_terminal_with(&content, &opts).unwrap());
        }
    }

    let plain: String = strip_ansi(&rendered);
    assert!(
        plain.contains("5. First"),
        "missing item 5.\nRendered: {plain:?}"
    );
    assert!(
        plain.contains("6. Second"),
        "missing renumbered item 6.\nRendered: {plain:?}"
    );
    assert!(
        plain.contains("7. Third without trailing newline"),
        "missing renumbered item 7.\nRendered: {plain:?}"
    );
}

#[test]
fn test_buffer_flush_event_preserves_continuation_paragraph_indent() {
    // Stream ends with a loose-item continuation paragraph (after a
    // blank line, indented to content_column). flush_event treats the
    // remaining buffer as an item flush (it starts with the item's
    // own marker line) and strips only `marker_column` leading spaces.
    // The continuation paragraph keeps its `content_column` indent,
    // which comrak then recognises as a loose-list continuation.
    let input = "1. Item one\n\n   continuation paragraph without trailing newline";
    let mut buf = Buffer::new();
    buf.push(input);
    let events: Vec<Event> = buf.by_ref().collect();

    assert!(events.is_empty(), "unexpected events: {events:?}");

    let flushed = buf.flush_events();
    assert_eq!(flushed, vec![Event::Flush {
        content: "1. Item one\n\n   continuation paragraph without trailing newline".into(),
        indent: 0,
    }]);
}

#[test]
fn test_buffer_continuation_paragraph_renders_at_content_column() {
    // End-to-end guarantee for the test above: the continuation
    // paragraph renders at column 3 (the item's content_column),
    // because comrak parses the partial flush as a loose-list item.
    use crate::format::{Formatter, TerminalOptions};

    let input = "1. Item one\n\n   continuation paragraph without trailing newline";
    let mut buf = Buffer::new();
    buf.push(input);
    let f = Formatter::with_width(0);
    let mut rendered = String::new();
    for ev in buf.flush_events() {
        if let Event::Block { content, indent } | Event::Flush { content, indent } = ev {
            let opts = TerminalOptions {
                indent,
                ..Default::default()
            };
            rendered.push_str(&f.format_terminal_with(&content, &opts).unwrap());
        }
    }

    let plain = strip_ansi(&rendered);
    let lines: Vec<&str> = plain.lines().collect();
    assert!(
        lines.iter().any(|l| l.starts_with("1. Item one")),
        "missing item marker line.\nRendered: {plain:?}"
    );
    assert!(
        lines
            .iter()
            .any(|l| l.starts_with("   continuation paragraph")),
        "continuation paragraph should render at column 3.\nRendered: {plain:?}"
    );
}

/// Strip ANSI escape sequences for plain-text assertions.
fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_escape = false;
    for c in s.chars() {
        if in_escape {
            if c.is_ascii_alphabetic() || c == '~' {
                in_escape = false;
            }
        } else if c == '\x1b' {
            in_escape = true;
        } else {
            out.push(c);
        }
    }
    out
}

#[test]
fn test_buffer_triple_nested_lists_stream_at_each_level() {
    // Three levels of ordered list nesting. Each level's items emit
    // their own Block with the level's `marker_column` as visual
    // indent, and ordered markers are renumbered per-level.
    let input = "1. Top\n   1. Mid one\n      1. Inner one\n      9. Inner two\n   3. Mid two\n2. \
                 Top two\n";
    let mut buf = Buffer::new();
    buf.push(input);
    let events: Vec<Event> = buf.by_ref().collect();

    assert_eq!(events, vec![
        Event::Block {
            content: "1. Top\n".into(),
            indent: 0,
        },
        Event::Block {
            content: "1. Mid one\n".into(),
            indent: 3,
        },
        Event::Block {
            content: "1. Inner one\n".into(),
            indent: 6,
        },
        Event::Block {
            content: "2. Inner two\n".into(),
            indent: 6,
        },
        Event::Block {
            content: "2. Mid two\n".into(),
            indent: 3,
        },
    ]);
    assert_eq!(buf.flush_events(), vec![Event::Flush {
        content: "2. Top two\n".into(),
        indent: 0
    }]);
}

#[test]
fn test_buffer_mixed_bullet_and_ordered_nesting() {
    // Mixed bullet outer, ordered inner.
    let input = "- Outer one\n  1. Inner one\n  5. Inner two\n- Outer two\n";
    let mut buf = Buffer::new();
    buf.push(input);
    let events: Vec<Event> = buf.by_ref().collect();

    assert_eq!(events, vec![
        Event::Block {
            content: "- Outer one\n".into(),
            indent: 0,
        },
        Event::Block {
            content: "1. Inner one\n".into(),
            indent: 2,
        },
        Event::Block {
            content: "2. Inner two\n".into(),
            indent: 2,
        },
    ]);
    assert_eq!(buf.flush_events(), vec![Event::Flush {
        content: "- Outer two\n".into(),
        indent: 0
    }]);
}

#[test]
fn test_buffer_ordered_outer_bullet_inner_nesting() {
    // Ordered outer, bullet inner. Bullets don't renumber; just
    // indent at the outer's content_column.
    let input = "1. Outer one\n   - Inner a\n   - Inner b\n2. Outer two\n";
    let mut buf = Buffer::new();
    buf.push(input);
    let events: Vec<Event> = buf.by_ref().collect();

    assert_eq!(events, vec![
        Event::Block {
            content: "1. Outer one\n".into(),
            indent: 0,
        },
        Event::Block {
            content: "- Inner a\n".into(),
            indent: 3,
        },
        Event::Block {
            content: "- Inner b\n".into(),
            indent: 3,
        },
    ]);
    assert_eq!(buf.flush_events(), vec![Event::Flush {
        content: "2. Outer two\n".into(),
        indent: 0
    }]);
}

#[test]
fn test_buffer_triple_nested_pop_does_not_emit_empty_block() {
    // Regression: when a deeply nested list reached the parent list's
    // next marker at the head of the buffer, the `Terminator` arm in
    // `handle_in_list` called `flush_list_segment(scan=0, ...)`, which
    // drained nothing and emitted `Event::Block { content: "", indent
    // = content_column }`. Now `scan == 0` pops back to the parent
    // without emitting anything.
    let input = "- Item B\n  - Nested B.1\n    - Deeply nested\n- Item C\n";
    let mut buf = Buffer::new();
    buf.push(input);
    let events: Vec<Event> = buf.by_ref().collect();

    assert_eq!(events, vec![
        Event::Block {
            content: "- Item B\n".into(),
            indent: 0,
        },
        Event::Block {
            content: "- Nested B.1\n".into(),
            indent: 2,
        },
        Event::Block {
            content: "- Deeply nested\n".into(),
            indent: 4,
        },
    ]);
    // No empty `Block { content: "", indent: 4 }` (or 2) between
    // `- Deeply nested` and `- Item C`.
    assert_eq!(buf.flush_events(), vec![Event::Flush {
        content: "- Item C\n".into(),
        indent: 0,
    }]);
}

#[test]
fn test_buffer_mixed_marker_at_same_column_starts_new_list() {
    // Per CommonMark §5.2, two markers are siblings only if they share
    // the same kind (bullet vs ordered) and delimiter character. A
    // mismatched marker at the same column starts a *new* list.
    //
    // Regression: the classifier used to treat any marker at the
    // current marker_column as a sibling, which incorrectly absorbed
    // the bullet into the ordered list and bumped `items_flushed`,
    // causing the subsequent `2. Two\n` to renumber to `3.`.
    let input = "1. One\n- Bullet\n2. Two\n";
    let mut buf = Buffer::new();
    buf.push(input);
    let events: Vec<Event> = buf.by_ref().collect();

    assert_eq!(events, vec![
        Event::Block {
            content: "1. One\n".into(),
            indent: 0,
        },
        Event::Block {
            content: "- Bullet\n".into(),
            indent: 0,
        },
    ]);
    // The trailing `2. Two\n` is its own ordered list (start_number=2)
    // with `items_flushed=0`, so it renumbers to itself — not to `3.`.
    assert_eq!(buf.flush_events(), vec![Event::Flush {
        content: "2. Two\n".into(),
        indent: 0,
    }]);
}

#[test]
fn test_buffer_different_ordered_delimiter_starts_new_list() {
    // `1.` and `2)` are different list types per CommonMark §5.2 —
    // they share `is_ordered` but use different delimiters.
    let input = "1. One\n2) Two\n";
    let mut buf = Buffer::new();
    buf.push(input);
    let events: Vec<Event> = buf.by_ref().collect();

    assert_eq!(events, vec![Event::Block {
        content: "1. One\n".into(),
        indent: 0,
    }]);
    assert_eq!(buf.flush_events(), vec![Event::Flush {
        content: "2) Two\n".into(),
        indent: 0,
    }]);
}

#[test]
fn test_buffer_different_bullet_char_starts_new_list() {
    // `-`, `*`, `+` are distinct bullet kinds per CommonMark §5.2.
    let input = "- One\n* Two\n";
    let mut buf = Buffer::new();
    buf.push(input);
    let events: Vec<Event> = buf.by_ref().collect();

    assert_eq!(events, vec![Event::Block {
        content: "- One\n".into(),
        indent: 0,
    }]);
    assert_eq!(buf.flush_events(), vec![Event::Flush {
        content: "* Two\n".into(),
        indent: 0,
    }]);
}

#[test]
fn test_buffer_two_blank_lines_terminate_list() {
    // Per CommonMark, two consecutive blank lines followed by
    // less-indented content end a list. The walk's `prev_blank` flag
    // is sticky across consecutive blanks, so we see this through:
    // blank → blank → non-marker at less indent → Terminator.
    //
    // The item Block keeps the trailing blank lines so the renderer
    // preserves the visual separator to the next Block. The same blank
    // lines also remain in the buffer (then consumed by the AtBoundary
    // trim before the paragraph is processed).
    let input = "1. Item\n\n\nparagraph at column 0\n\n";
    let mut buf = Buffer::new();
    buf.push(input);
    let events: Vec<Event> = buf.by_ref().collect();

    assert_eq!(events, vec![
        Event::Block {
            content: "1. Item\n\n\n".into(),
            indent: 0,
        },
        Event::block("paragraph at column 0\n\n"),
    ]);
    assert_eq!(buf.flush_events(), Vec::<Event>::new());
}

#[test]
fn test_buffer_unindented_paragraph_after_nested_list_terminates_outer() {
    // Regression: a non-marker line at column 0 after a blank line
    // must terminate the OUTER list, not be folded into the outer
    // item's lazy continuation. Previously, the nested list's flush
    // consumed the blank line that signalled termination, leaving the
    // outer state without `prev_blank=true`. The fix asymmetrically
    // captures the trailing blank in the Block content (so the
    // renderer keeps the visual separator) while leaving it in the
    // buffer (so the parent state picks up `prev_blank=true`), and
    // initialises `prev_blank` from any leading blank consumed at
    // entry to `handle_in_list`.
    let input = "- Outer\n  - Inner\n\nparagraph\n";
    let mut buf = Buffer::new();
    buf.push(input);
    let mut events: Vec<Event> = buf.by_ref().collect();
    events.extend(buf.flush_events());

    assert_eq!(events, vec![
        Event::Block {
            content: "- Outer\n".into(),
            indent: 0,
        },
        Event::Block {
            content: "- Inner\n\n".into(),
            indent: 2,
        },
        Event::Flush {
            content: "paragraph\n".into(),
            indent: 0,
        },
    ]);
}

#[test]
fn test_buffer_unindented_paragraph_after_nested_list_with_following_block() {
    // Same as above, but with a trailing top-level heading so the
    // paragraph is emitted as a streaming `Block`, not a `Flush`. This
    // exercises the path where `handle_in_list` pops to `AtBoundary`
    // and `handle_buffering_paragraph` collects the paragraph.
    let input = "- Outer\n  - Inner\n\nparagraph at column 0\n\n# Heading\n";
    let mut buf = Buffer::new();
    buf.push(input);
    let mut events: Vec<Event> = buf.by_ref().collect();
    events.extend(buf.flush_events());

    assert_eq!(events, vec![
        Event::Block {
            content: "- Outer\n".into(),
            indent: 0,
        },
        Event::Block {
            content: "- Inner\n\n".into(),
            indent: 2,
        },
        Event::Block {
            content: "paragraph at column 0\n\n".into(),
            indent: 0,
        },
        Event::Block {
            content: "# Heading\n".into(),
            indent: 0,
        },
    ]);
}

#[test]
fn test_buffer_terminated_list_renders_blank_before_next_block() {
    // Regression: an earlier iteration of the terminator fix stripped
    // the trailing blank from the last item's Block, which collapsed
    // into the next Block at render time because the renderer only
    // emits a trailing blank when the source had one (list items don't
    // auto-add a blank like paragraphs do). The Block must keep the
    // trailing blank in its content.
    use crate::format::{Formatter, TerminalOptions};

    let cases = [
        // List → paragraph.
        (
            "1. a\n2. b\n3. c\n\nParagraph after.\n",
            "3. c",
            "Paragraph after.",
        ),
        // List → heading.
        ("- a\n- b\n\n## Heading\n", "- b", "## Heading"),
    ];

    let f = Formatter::with_width(0);
    for (input, last_item, next_line) in cases {
        let mut buf = Buffer::new();
        buf.push(input);
        let mut events: Vec<Event> = buf.by_ref().collect();
        events.extend(buf.flush_events());

        let mut rendered = String::new();
        for ev in &events {
            if let Event::Block { content, indent } | Event::Flush { content, indent } = ev {
                let opts = TerminalOptions {
                    indent: *indent,
                    ..Default::default()
                };
                rendered.push_str(&f.format_terminal_with(content, &opts).unwrap());
            }
        }

        let plain = strip_ansi(&rendered);
        let lines: Vec<&str> = plain.lines().collect();
        let last_idx = lines
            .iter()
            .position(|l| *l == last_item)
            .unwrap_or_else(|| panic!("missing `{last_item}`.\nRendered:\n{plain}"));
        assert_eq!(
            lines.get(last_idx + 1),
            Some(&""),
            "blank line missing between `{last_item}` and `{next_line}`.\nRendered:\n{plain}"
        );
    }
}

#[test]
fn test_buffer_unindented_paragraph_after_nested_list_renders_at_column_0() {
    // End-to-end guarantee: the user's reported input renders the
    // trailing paragraph flush left, not indented as a continuation
    // of the outer item.
    use crate::format::{Formatter, TerminalOptions};

    let input = "- Outer\n  - Inner\n\nparagraph\n\n# Heading\n";
    let mut buf = Buffer::new();
    buf.push(input);
    let mut events: Vec<Event> = buf.by_ref().collect();
    events.extend(buf.flush_events());

    let f = Formatter::with_width(0);
    let mut rendered = String::new();
    for ev in &events {
        if let Event::Block { content, indent } | Event::Flush { content, indent } = ev {
            let opts = TerminalOptions {
                indent: *indent,
                ..Default::default()
            };
            rendered.push_str(&f.format_terminal_with(content, &opts).unwrap());
        }
    }

    let plain = strip_ansi(&rendered);
    assert!(
        plain.lines().any(|l| l == "paragraph"),
        "`paragraph` should render at column 0.\nRendered:\n{plain}"
    );
    assert!(
        !plain.lines().any(|l| l == "  paragraph"),
        "`paragraph` must NOT render at column 2 as a continuation.\nRendered:\n{plain}"
    );
}

#[test]
fn test_buffer_indented_paragraph_after_nested_list_stays_continuation() {
    // Sibling case to the above: when the paragraph IS indented to the
    // outer item's `content_column`, it remains a continuation of the
    // outer item per CommonMark §5.2 and renders at that indent. The
    // fix must not accidentally promote legitimate continuations.
    //
    // The nested item's Block keeps the trailing blank in its content
    // (so the renderer preserves the separator) while leaving the same
    // blank in the buffer for the outer to consume as a leading blank.
    // The outer's continuation Flush sits at the outer item's
    // `content_column` of 2.
    let input = "- Outer\n  - Inner\n\n  Continuation of outer\n";
    let mut buf = Buffer::new();
    buf.push(input);
    let mut events: Vec<Event> = buf.by_ref().collect();
    events.extend(buf.flush_events());

    assert_eq!(events, vec![
        Event::Block {
            content: "- Outer\n".into(),
            indent: 0,
        },
        Event::Block {
            content: "- Inner\n\n".into(),
            indent: 2,
        },
        Event::Flush {
            content: "Continuation of outer\n".into(),
            indent: 2,
        },
    ]);
}

#[test]
fn test_buffer_block_interrupter_inside_list_terminates() {
    // A block interrupter (header, fence, thematic break, HTML block)
    // at <=3 indent terminates the list, even without a preceding
    // blank line.
    let input = "1. Item\n# Header that interrupts\n";
    let mut buf = Buffer::new();
    buf.push(input);
    let events: Vec<Event> = buf.by_ref().collect();

    assert_eq!(events, vec![
        Event::Block {
            content: "1. Item\n".into(),
            indent: 0,
        },
        Event::block("# Header that interrupts\n"),
    ]);
    assert_eq!(buf.flush_events(), Vec::<Event>::new());
}

#[test]
fn test_buffer_lazy_continuation_inside_list_item() {
    // CommonMark lazy continuation: a non-blank line at less indent
    // than content_column, NOT preceded by a blank line, is part of
    // the current item's paragraph.
    let input = "1. First line of item\nlazy continuation\n2. Next item\n";
    let mut buf = Buffer::new();
    buf.push(input);
    let events: Vec<Event> = buf.by_ref().collect();

    // The first item bundles the lazy continuation line; only the
    // sibling marker triggers the flush.
    assert_eq!(events, vec![Event::Block {
        content: "1. First line of item\nlazy continuation\n".into(),
        indent: 0,
    }]);
    assert_eq!(buf.flush_events(), vec![Event::Flush {
        content: "2. Next item\n".into(),
        indent: 0
    }]);
}

#[test]
fn test_buffer_nested_list_streams_with_renumbering() {
    // Sub-items emit individually with the parent's content column as
    // visual indent. Ordered markers are renumbered relative to the
    // nested list's start number, so `1, 7, 99` renders as `1, 2, 3`.
    let input = "1. Outer\n   1. Sub one\n   7. Sub two\n   99. Sub three\n2. Next outer\n";
    let mut buf = Buffer::new();
    buf.push(input);
    let events: Vec<Event> = buf.by_ref().collect();

    assert_eq!(events, vec![
        Event::Block {
            content: "1. Outer\n".into(),
            indent: 0,
        },
        Event::Block {
            content: "1. Sub one\n".into(),
            indent: 3,
        },
        Event::Block {
            content: "2. Sub two\n".into(),
            indent: 3,
        },
        Event::Block {
            content: "3. Sub three\n".into(),
            indent: 3,
        },
    ]);
    assert_eq!(buf.flush_events(), vec![Event::Flush {
        content: "2. Next outer\n".into(),
        indent: 0
    }]);
}

#[test]
fn test_buffer_fence_in_list_streams_with_indent() {
    // A fenced code block opened inside a list item is recognised and
    // streams as `FencedCode*` events with the fence's visual column as
    // its `indent`. Item content emitted around it uses the same indent
    // logic as the rest of the list.
    let input = "1. Here's code:\n\n   ```rust\n   fn main() {}\n   ```\n\n2. Next item.\n";
    let mut buf = Buffer::new();
    buf.push(input);
    let events: Vec<Event> = buf.by_ref().collect();

    assert_eq!(events, vec![
        Event::Block {
            content: "1. Here's code:\n\n".into(),
            indent: 0,
        },
        Event::FencedCodeStart {
            language: "rust".into(),
            fence_type: FenceType::Backtick,
            fence_length: 3,
            indent: 3,
        },
        Event::FencedCodeLine {
            content: "fn main() {}\n".into(),
            indent: 3,
        },
        Event::FencedCodeEnd {
            fence: "```".into(),
            indent: 3,
        },
    ]);
    assert_eq!(buf.flush_events(), vec![Event::Flush {
        content: "2. Next item.\n".into(),
        indent: 0
    }]);
}

#[test]
fn test_buffer_fence_in_two_digit_item_closes() {
    // Regression: the closing-fence detector used the document-level
    // `indent_len < 4` rule, which is wrong for fences nested inside
    // list items with `content_column >= 4`. With marker `10. `,
    // content_column is 4, so the opening fence at indent=4 enters
    // `InFencedCode { indent: 4, .. }` — and the closing fence, also
    // at indent=4, would fail `indent_len < 4` and stay in the
    // `InFencedCode` state forever. The check is now relative to the
    // stored fence indent (`indent_len - indent < 4`), so this closes.
    let input = "10. Outer\n\n    ```rust\n    fn main() {}\n    ```\n\n11. Next\n";
    let mut buf = Buffer::new();
    buf.push(input);
    let events: Vec<Event> = buf.by_ref().collect();

    assert_eq!(events, vec![
        Event::Block {
            content: "10. Outer\n\n".into(),
            indent: 0,
        },
        Event::FencedCodeStart {
            language: "rust".into(),
            fence_type: FenceType::Backtick,
            fence_length: 3,
            indent: 4,
        },
        Event::FencedCodeLine {
            content: "fn main() {}\n".into(),
            indent: 4,
        },
        Event::FencedCodeEnd {
            fence: "```".into(),
            indent: 4,
        },
    ]);
    assert_eq!(buf.flush_events(), vec![Event::Flush {
        content: "11. Next\n".into(),
        indent: 0,
    }]);
}

#[test]
fn test_buffer_fence_in_list_allows_extra_close_indent() {
    // CommonMark §4.5 allows the closing fence to be up to 3 spaces
    // more indented than the opening fence's container. The opening
    // fence here sits at column 3; the closer at column 6 (three
    // extra spaces) must still close it.
    let input = "1. item\n\n   ```rust\n   fn x() {}\n      ```\n";
    let mut buf = Buffer::new();
    buf.push(input);
    let events: Vec<Event> = buf.by_ref().collect();

    assert!(
        events
            .iter()
            .any(|e| matches!(e, Event::FencedCodeEnd { .. })),
        "closing fence with up-to-3-extra indent should be recognised. Got: {events:#?}"
    );
}

#[test]
fn test_buffer_loose_list_with_indented_nested_content_not_split() {
    // Regression for the original loose-list bug, updated to the
    // Option A semantics: the outer marker line, each nested sub-item,
    // and the trailing paragraph each stream as their own `Block` with
    // the appropriate visual `indent`. Ordered sub-item markers are
    // renumbered relative to the nested list's start number.
    let input = "10. **Outer item** \u{2014}\n\n    1. Sub one\n    7. Sub two\n    99. Sub \
                 three\n\n    End of item 10.\n\n11. **Next outer**\n";

    let mut buf = Buffer::new();
    buf.push(input);
    let events: Vec<Event> = buf.by_ref().collect();

    assert_eq!(events, vec![
        Event::Block {
            content: "10. **Outer item** \u{2014}\n\n".into(),
            indent: 0,
        },
        Event::Block {
            content: "1. Sub one\n".into(),
            indent: 4,
        },
        Event::Block {
            content: "2. Sub two\n".into(),
            indent: 4,
        },
        Event::Block {
            content: "3. Sub three\n\n".into(),
            indent: 4,
        },
        Event::Block {
            content: "End of item 10.\n\n".into(),
            indent: 4,
        },
    ]);
    assert_eq!(buf.flush_events(), vec![Event::Flush {
        content: "11. **Next outer**\n".into(),
        indent: 0
    }]);
}

#[test]
fn test_buffer_list_streams_incrementally() {
    // Bug: lists buffered entirely until a blank line, causing streaming
    // to stall for the duration of list generation. Now the buffer
    // flushes at each new top-level item boundary.
    let mut buf = Buffer::new();

    // Stream in a 4-item list, one line at a time.
    buf.push("1. First item\n");
    assert_eq!(buf.by_ref().collect::<Vec<_>>(), vec![]); // needs another item

    buf.push("2. Second item\n");
    let events: Vec<Event> = buf.by_ref().collect();
    assert_eq!(
        events,
        vec![Event::block("1. First item\n")],
        "Should flush first item when second arrives"
    );

    buf.push("3. Third item\n");
    let events: Vec<Event> = buf.by_ref().collect();
    assert_eq!(
        events,
        vec![Event::block("2. Second item\n")],
        "Should flush second item when third arrives"
    );

    // Blank line terminates the list.
    buf.push("\nAfter list\n\n");
    let events: Vec<Event> = buf.by_ref().collect();
    assert_eq!(events, vec![
        Event::block("3. Third item\n\n"),
        Event::block("After list\n\n"),
    ],);
}

#[test]
fn test_buffer_list_multiline_items_not_split() {
    // List items with continuation lines should not be split.
    let mut buf = Buffer::new();
    buf.push("1. First item that\n   continues here\n");
    buf.push("2. Second item\n\n");

    // First item (with continuation) flushes when the second item's
    // marker arrives. The second item is the last one we've seen so far
    // — a trailing blank line alone doesn't end the list, because more
    // indented content could still arrive.
    let events: Vec<Event> = buf.by_ref().collect();
    assert_eq!(events, vec![Event::block(
        "1. First item that\n   continues here\n"
    )]);

    // The list ends when non-indented, non-marker content appears after
    // the blank line.
    buf.push("Outside list\n");
    let events: Vec<Event> = buf.by_ref().collect();
    assert_eq!(events, vec![Event::block("2. Second item\n\n")]);
}

#[test]
fn test_buffer_unordered_list_streams() {
    let mut buf = Buffer::new();
    buf.push("- Alpha\n- Beta\n- Gamma\n\n");

    // Items stream at each sibling marker. The last item stays buffered
    // until the list is known to have ended.
    let events: Vec<Event> = buf.by_ref().collect();
    assert_eq!(events, vec![
        Event::block("- Alpha\n"),
        Event::block("- Beta\n"),
    ]);

    // End-of-stream flushes the remaining item.
    assert_eq!(buf.flush_events(), vec![Event::Flush {
        content: "- Gamma\n\n".into(),
        indent: 0
    }]);
}

#[test]
fn test_buffer_thematic_break() {
    let cases = vec![
        ("simple", TestCase {
            in_out: vec![("---\n", vec![Event::block("---\n")])],
            flushed: None,
        }),
        ("with spaces", TestCase {
            in_out: vec![(" * * * \n", vec![Event::block(" * * * \n")])],
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
        in_out: vec![("[my-link]: https://example.com\n", vec![Event::block(
            "[my-link]: https://example.com\n",
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
                ("console.log(x);\n</script>\n", vec![Event::block(
                    "<script>\nvar x = 1;\n\nconsole.log(x);\n</script>\n",
                )]),
            ],
            flushed: None,
        }),
        ("Type 6 (Div) fragmented", TestCase {
            in_out: vec![
                ("<div>\n  <p>Hello</p>\n", vec![]),
                ("\nThis is after.", vec![Event::block(
                    "<div>\n  <p>Hello</p>\n\n",
                )]),
            ],
            flushed: Some("This is after."),
        }),
        ("Type 7 (No Interrupt)", TestCase {
            in_out: vec![
                ("This is a paragraph.\n", vec![]),
                ("<a>foo</a>\n\n", vec![Event::block(
                    "This is a paragraph.\n<a>foo</a>\n\n",
                )]),
            ],
            flushed: None,
        }),
        ("Type 2 (Comment)", TestCase {
            in_out: vec![
                ("<!-- Hello\n\n", vec![]),
                ("World -->\nPara\n\n", vec![
                    Event::block("<!-- Hello\n\nWorld -->\n"),
                    Event::block("Para\n\n"),
                ]),
            ],
            flushed: None,
        }),
        ("Type 4 (Doctype)", TestCase {
            in_out: vec![("<!DOCTYPE html>\n<p>Hi</p>\n\n", vec![
                Event::block("<!DOCTYPE html>\n"),
                Event::block("<p>Hi</p>\n\n"),
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
                Event::block("# Header 1\n"),
                Event::block("Paragraph.\n\n"),
                Event::block("---\n"),
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
                ("# Header\n", vec![Event::block("# Header\n")]),
                ("\n\nPara\n\n", vec![Event::block("Para\n\n")]),
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
    assert_eq!(actual, vec![Event::block("# Hello\n")]);

    let _ = writeln!(buf, "\nAnd a new one.");

    let actual: Vec<Event> = buf.by_ref().collect();
    assert_eq!(actual, vec![Event::block(
        "This is a paragraph.\nIt has two lines.\n\n"
    )]);

    assert_eq!(buf.flush_events(), vec![Event::Flush {
        content: "And a new one.\n".into(),
        indent: 0
    }]);
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

/// Characterization: `flush_events` on a mid-list buffer splits the remaining
/// content at item boundaries and renumbers ordered items, consistent with the
/// streaming path.
///
/// The multi-segment case is reachable in normal exhaust-then-flush usage: an
/// item cannot flush until its *next sibling's* line is complete, so a stream
/// ending mid-line leaves one complete item plus the partial tail queued.
#[test]
fn test_flush_events_mid_list_segments() {
    // Same-kind siblings: both segments renumbered against the list start.
    let mut buf = Buffer::new();
    buf.push("1. one\n5. two\n9. three");
    let streamed: Vec<Event> = buf.by_ref().collect();
    assert_eq!(streamed, vec![Event::block("1. one\n")]);
    assert_eq!(buf.flush_events(), vec![
        Event::block("2. two\n"),
        Event::flush("3. three"),
    ]);

    // Mixed delimiter in the tail: `9) three` is not a sibling of the `.`
    // list. It still marks a segment boundary, but must not be renumbered.
    let mut buf = Buffer::new();
    buf.push("1. one\n5. two\n9) three");
    let streamed: Vec<Event> = buf.by_ref().collect();
    assert_eq!(streamed, vec![Event::block("1. one\n")]);
    assert_eq!(buf.flush_events(), vec![
        Event::block("2. two\n"),
        Event::flush("9) three"),
    ]);

    // Bullet list with an ordered tail: no renumbering anywhere.
    let mut buf = Buffer::new();
    buf.push("- one\n- two\n1. three");
    let streamed: Vec<Event> = buf.by_ref().collect();
    assert_eq!(streamed, vec![Event::block("- one\n")]);
    assert_eq!(buf.flush_events(), vec![
        Event::block("- two\n"),
        Event::flush("1. three"),
    ]);
}

/// A paragraph interruption decision must wait for the next line to be
/// complete: a partial prefix can look like a block starter (`#`, `<div`, `
/// ```a `) while the completed line is not one.
/// Chunked and whole-document parsing must agree.
#[test]
fn test_paragraph_interrupt_waits_for_complete_line() {
    let cases = vec![
        // "#" alone is a valid ATX header; "#hello" is not (no space).
        ("atx_prefix", "para\n#hello\n\nnext\n\n", vec![
            "para\n#",
            "hello\n\nnext\n\n",
        ]),
        // "```" alone opens a fence; "```a`b" does not (backtick in info).
        ("fence_info_backtick", "para\n```a`b\n\nnext\n\n", vec![
            "para\n```",
            "a`b\n\nnext\n\n",
        ]),
        // "<div" alone is an HTML type-6 starter; "<divx" is not a known
        // tag (and type 7 cannot interrupt a paragraph).
        ("html_tag_prefix", "para\n<divx>\n\nnext\n\n", vec![
            "para\n<div",
            "x>\n\nnext\n\n",
        ]),
    ];

    for (name, whole, chunks) in cases {
        let mut whole_buf = Buffer::from(whole);
        let mut expected: Vec<Event> = whole_buf.by_ref().collect();
        expected.extend(whole_buf.flush_events());

        let mut chunked_buf = Buffer::new();
        let mut actual = Vec::new();
        for chunk in chunks {
            chunked_buf.push(chunk);
            actual.extend(chunked_buf.by_ref());
        }
        actual.extend(chunked_buf.flush_events());

        assert_eq!(actual, expected, "failed case: {name}");
    }
}

#[test]
fn test_buffer_atx_header_validation() {
    // Invalid headers treated as paragraphs
    let invalid = vec![
        ("#hashtag\n\n", vec![Event::block("#hashtag\n\n")]),
        ("#5 bolt\n\n", vec![Event::block("#5 bolt\n\n")]),
        ("####### foo\n\n", vec![Event::block("####### foo\n\n")]),
    ];

    for (input, expected) in invalid {
        let mut buf = Buffer::new();
        buf.push(input);
        let actual: Vec<Event> = buf.by_ref().collect();
        assert_eq!(actual, expected, "Failed for input: {input:?}");
    }

    // Valid headers
    let valid = vec![
        ("# Valid\n", vec![Event::block("# Valid\n")]),
        ("###### Six\n", vec![Event::block("###### Six\n")]),
        ("#\tTab\n", vec![Event::block("#\tTab\n")]),
        ("#\n", vec![Event::block("#\n")]),
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
    assert_eq!(buf.flush_events(), vec![Event::Flush {
        content: "\t# Not Header\n\n".into(),
        indent: 0
    }]);

    // 3 spaces before # = valid header
    let mut buf = Buffer::new();
    buf.push("   # Valid\n");
    let actual: Vec<Event> = buf.by_ref().collect();
    assert_eq!(actual, vec![Event::block("   # Valid\n")]);

    // Tab after # = valid header
    let mut buf = Buffer::new();
    buf.push("#\tFoo\n");
    let actual: Vec<Event> = buf.by_ref().collect();
    assert_eq!(actual, vec![Event::block("#\tFoo\n")]);

    // Tab at column 0 before thematic break = indented code
    let mut buf = Buffer::new();
    buf.push("\t***\n\n");
    let actual: Vec<Event> = buf.by_ref().collect();
    assert_eq!(actual, Vec::<Event>::new());
    assert_eq!(buf.flush_events(), vec![Event::Flush {
        content: "\t***\n\n".into(),
        indent: 0
    }]);

    // 3 spaces before *** = valid thematic break
    let mut buf = Buffer::new();
    buf.push("   ***\n");
    let actual: Vec<Event> = buf.by_ref().collect();
    assert_eq!(actual, vec![Event::block("   ***\n")]);

    // Mixed tabs and spaces in thematic break
    let mut buf = Buffer::new();
    buf.push("*\t*\t*\t\n");
    let actual: Vec<Event> = buf.by_ref().collect();
    assert_eq!(actual, vec![Event::block("*\t*\t*\t\n")]);
}

#[test]
fn test_buffer_event_display() {
    let cases = vec![
        (Event::block("Hello"), "Hello"),
        (
            Event::FencedCodeStart {
                language: "rust".into(),
                fence_type: FenceType::Backtick,
                fence_length: 3,
                indent: 0,
            },
            "```rust",
        ),
        (
            Event::FencedCodeStart {
                language: "python".into(),
                fence_type: FenceType::Tilde,
                fence_length: 4,
                indent: 0,
            },
            "~~~~python",
        ),
        (Event::fenced_code_line("Hello"), "Hello"),
        (Event::fenced_code_end("```"), "```"),
    ];

    for (event, expected) in cases {
        assert_eq!(event.to_string(), expected);
    }
}
