# file:// attachments misread paths through the URL host

- **Status**: In Progress
- **Authors**: jp
- **Date**: 2026-11-25
- **Label**: client=cli
- **Label**: package=jp_attachment
- **Label**: package=jp_attachment_file_content
- **Label**: package=jp_cli
- **Label**: package=jp_config
- **Label**: type=bug

The `file` handler reads `file://docs/notes.md` as the relative path
`docs/notes.md`: it treats the URL's host as the first path segment
(`uri_to_pattern` in `crates/jp_attachment_file_content/src/lib.rs`, and the
crate README).
That spelling cannot work in general, because the URL parser normalizes a `file`
URL's host before any handler sees it.

What `url::Url` hands the handler, and the pattern the handler builds from it:

| Written                     | Parsed                    | Pattern             | Effect                                    |
| --------------------------- | ------------------------- | ------------------- | ----------------------------------------- |
| `file://notes.md`           | host `notes.md`, path `/` | `/notes.md/`        | attaches nothing                          |
| `file://Docs/Notes.md`      | host `docs`               | `/docs/Notes.md`    | wrong file on a case-sensitive filesystem |
| `file://café/x.md`          | host `xn--caf-dma`        | `/xn--caf-dma/x.md` | attaches nothing                          |
| `file://localhost/notes.md` | no host                   | `/notes.md`         | a directory named `localhost` is dropped  |
| `file://my dir/x.md`        | parse error               |                     | rejected                                  |

The first row is a handler bug.
The rest is information the parser discards, so no change to the handler can
recover it.

The host form is not only something users type.
JP produces it in three places:

- `AttachmentObjectConfig::to_url`
  (`crates/jp_config/src/conversation/attachment.rs`) builds `<type>://<path>`,
  so every `type = "file"` config table becomes the host form.
  `path = "notes.md"` therefore attaches nothing.
- `.jp/config/skill/rfd.toml` attaches `file://docs/rfd/001-jp-rfd-process.md`,
  and the `justfile` passes `--attach "file://$file"`.
- The crate README documents `file://path/to/file.txt`, `file://**/*.md`, and
  `file://path/to/file.txt?exclude`.

The CLI's own path parser (`AttachmentUrlOrPath::parse`) already writes
`file:<path>`, which keeps the path in the URL path and is lossless.

## Fix

Keep paths out of the authority wherever JP builds a `file` URL, and read the
host form only for what is already stored.

1. **Handler bug:** in `uri_to_pattern`, a host with path `/` is the host alone,
   so `file://notes.md` reads as `notes.md`.
   This is compatible, and fixes stored entries and tables already written that
   way.
2. **Config tables:** let the handler build the URL for a table rather than the
   generic `<type>://<path>`.
   Add a `Handler` method (named in the PR; `from_table(path, params) ->
   Result<Url, _>`, say) whose default is the current generic build.
   `file` overrides it to put `path` in the URL path (`file:///<path>`).
   `jp_cli` converts tables through it wherever it calls
   `AttachmentConfig::to_url` today (`TurnInputs::collect`, `attach_urls`,
   `attachment rm`/`ls`, `conversation show`).
   `jp_config` stays free of handler knowledge; whether `to_url` stays there as
   the generic fallback or moves is a choice for the PR.
3. **Producers in the repo:** rewrite `rfd.toml`, the `justfile` recipe, and the
   README examples to `file:<path>`.
4. **User-typed host form:** keep resolving it, with item 1's fix, so stored
   conversations keep working.
   When `canonical` sees a host, log a warning that names the lossless spelling
   (`file:Docs/Notes.md`), since it cannot tell whether the host was lowercased.
   Rejecting the form outright would also be sound, but it breaks every
   conversation that stores one and has no migration path yet (see T-1rm5g3a).

The table conversion in item 2 is the only part that touches the `Handler`
trait.
Items 1 and 3 stand alone and can land first.

- [x] `file://notes.md` attaches `notes.md`, with a regression test
- [x] A `type = "file"` table with a mixed-case or non-ASCII `path` attaches
      that exact file
- [x] `rfd.toml`, the `justfile`, and the `file` README no longer write the host
      form
- [x] A host-form `file` URL logs a warning naming the lossless spelling
- [x] `canonical` of a table and of the equivalent `file:<path>` URL are equal
