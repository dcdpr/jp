# Attachment: Agentic Shepherd

This crate provides an attachment handler for issues tracked by
`agentic-shepherd`, a local issue tracker. It resolves an issue reference to a
markdown attachment by running the `agentic-shepherd` binary and rendering its
JSON output.

## URI Format

The handler owns the `ag` scheme. All of the following refer to issue `592`:

- `ag://issues/592`
- `ag:issues/592`
- `ag://issue/592`
- `ag:issue/592`
- `ag://592`
- `ag:592`

`issues` is a namespace; the singular and plural spellings are equivalent. A
bare number (`ag:592`) defaults to the `issues` namespace. Issue IDs are
numeric.

## Usage

```sh
jp attachment add ag:592
jp attachment add ag://issues/592
```

When the conversation is sent, the handler runs the following from the
workspace root:

```sh
agentic-shepherd --json '{"command": "IssueDetail", "issue_id_str": "592"}'
```

It parses the returned issue and renders, in order, the title, description, and
any populated sections (analysis, implementation plan, progress notes,
implementation details, testing results, debugging), followed by the
resolution. Nested outline structure, code blocks, checklists, and test results
are preserved in the markdown.

The `agentic-shepherd` binary must be available on `PATH`. A missing binary, a
non-zero exit, or unparseable output fails the attachment loudly rather than
attaching nothing.
