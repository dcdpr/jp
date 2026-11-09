# comfort pads inline code that starts or ends with a space

- **Status**: Done
- **Authors**: jp
- **Date**: 2026-11-07
- **Label**: domain=tooling
- **Label**: package=comfort
- **Label**: type=bug

With `format_markdown = true`, comfort formats Markdown through comrak's
`format_commonmark`, which adds a space inside each backtick delimiter whenever
an inline code span starts or ends with a space:

```text
`# `    becomes    ` #  `
`a `    becomes    ` a  `
` a`    becomes    `  a `
```

CommonMark strips one leading and one trailing space from a code span that has a
space at both ends, so each pair renders the same HTML.
In the Markdown source, however, the padded span looks like a different literal
in the file and in diffs, and deleting the apparent extra space by hand changes
the rendered output.

Found while amending RFD 121, where a code span containing `#` and a trailing
space was padded and had to be reworded.
The same padding appeared in this ticket's first draft.
With default options comfort leaves code spans untouched; only `format_markdown`
runs comrak's writer.

## Fix

Pad code-span delimiters with a space only when CommonMark requires it: when the
literal starts or ends with a backtick, or starts and ends with a space around
non-space content.
Comfort already post-processes comrak's output to restore reference links, so
code-span rewriting can run in the same pipeline.
Add tests covering the three cases above and verifying that the rendered HTML is
unchanged.
