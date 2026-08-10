# T0003: rfd-lint has no tests

- **Status**: Todo
- **Kind**: Chore
- **Authors**: jp
- **Date**: 2026-08-10

`docs/.vitepress/rfd-lint.mjs` is roughly 500 lines of parsing and rule
evaluation with no test coverage. It gates `just rfd-promote` and runs inside
every `just rfd-cycle` round, so a wrong answer changes what gets written.

It has already shipped two bugs found only by reading its output on real
documents:

- Sentence-shaped rules counted wrapped lines rather than sentences, because
  `comfort` wraps at 80 columns. RFD 001's three-sentence Summary reported as
  six, a paragraph reported 13 sentences, and `duplicate` silently compared line
  fragments and so never fired. Fixed by joining consecutive prose lines into
  blocks first.
- A clean file printed no word count, which is the number the author persona is
  instructed to report.

Both would have been caught by a fixture.

## What to cover

- `classify` and `blocks`: fenced code, HTML comments, the metadata header,
  tables, reference definitions, list items with wrapped continuations.
- Prose word counting, with code blocks and metadata excluded.
- Each rule against a fixture that triggers it and one that does not.
- Severity: budget and Non-Goals gate in Draft and Discussion and warn
  afterwards; line-level rules never gate.
- `--json`, `--summary`, `--since`, `--errors-only`, and the exit code.

## Where

The project already runs node checks in the website workflow. A sibling
`rfd-lint.test.mjs` using `node:test` keeps the dependency count at zero, and
wires into `docs-ci` next to `rfd-lint-ci`.

## Prior art

A parallel effort on this problem shipped node checker tests as part of its CI
and verified them before landing. This is the piece of that work worth copying
outright.
