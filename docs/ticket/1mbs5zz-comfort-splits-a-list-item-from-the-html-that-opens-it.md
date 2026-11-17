# comfort splits a list item from the HTML that opens it

- **Status**: Done
- **Authors**: jp
- **Date**: 2026-11-17
- **Label**: domain=tooling
- **Label**: package=comfort
- **Label**: type=bug

comfort's Markdown formatting writes a list item whose first block is HTML, such
as a comment, as its marker alone, with a trailing space, and the HTML indented
on the line below:

```markdown
- 
  <!-- C-1a2b3c4 -->
  The entry.
```

The split comes from comrak's CommonMark writer, which starts an HTML block with
a blank line even as the first block of an item; it already skips that for a
code block there.

At the top of a list the split form is only untidy: it parses as the same item,
but leaves trailing whitespace.
As the first item of a list that follows a paragraph line, such as a nested list
under its parent's text, it changes the document: an item that starts blank
can't interrupt a paragraph, so the marker reads as text and the item is lost.

```markdown
- An entry.
  - <!-- C-5d6e7f8 -->
    A nested entry.
```

Release notes give each entry an identity in a comment that opens its item (RFD
122), so comfort has to keep the comment on the marker line.

- [x] Keep HTML that opens a list item on the item's marker line, after block
      quotes, nested items, and ordered markers.
- [x] Leave a marker line inside a code or HTML block as it is.
