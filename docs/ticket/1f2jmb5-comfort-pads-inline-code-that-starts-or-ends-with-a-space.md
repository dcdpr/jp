# comfort pads inline code that starts or ends with a space

- **Status**: Done
- **Authors**: jp
- **Date**: 2026-11-07
- **Label**: domain=tooling
- **Label**: package=comfort
- **Label**: type=bug

With `format_markdown = true`, comfort lays Markdown out through comrak's
`format_commonmark`, which pads an inline code span whose text starts or ends
with a space with one more space on each side:

```text
`# `    becomes    ` #  `
`a `    becomes    ` a  `
` a`    becomes    `  a `
```

CommonMark strips one space from each side of a code span that both starts and
ends with one, so each pair renders the same HTML.
The source doesn't read the same, though: the padded span looks like a different
string to anyone reading the file or its diff, and trimming the "extra" space by
hand changes what renders.

Found while amending RFD 121, where a code span holding `#` and one trailing
space came out padded and had to be reworded.
The same thing happened to this ticket's first draft when it was written.
With default options comfort leaves the spans alone; only `format_markdown` goes
through comrak's writer.

## Fix

Write a code span with padding only where CommonMark needs it: when its text
starts or ends with a backtick, or both starts and ends with a space.
Comfort already post-processes comrak's output, restoring reference links, so
this can happen in the same place.
Cover the three cases above, and that each still renders as its input does.
