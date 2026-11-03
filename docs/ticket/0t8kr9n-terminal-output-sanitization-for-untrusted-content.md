# Terminal Output Sanitization for Untrusted Content

- **Status**: Todo
- **Kind**: Feature
- **Authors**: Jean Mertz <git@jeanmertz.com>
- **Date**: 2026-09-28
- **Implements**: 096
- **Label**: type=tracking

Tracking ticket for [RFD 096].

## Implementation plan

- **Build ContentWriter allowlist filter**: Add
  `jp_term::sanitize::ContentWriter`, the vte-based per-class SGR allowlist
  filter with span-closing reset and shared conceal removal, plus unit tests for
  sequence classes, chunk-split sequences, and `finish()` behavior.
  Standalone, no behavior change until wired.
- **Close tool-output spans for #1201**: Route tool-result bytes in
  `jp_cli::render::tool` (`render_result`, `render_formatted_arguments`) through
  `ContentWriter` under pass-through (`off`) policy, closing the span before the
  truncation note.
  Fixes the bleeding-color regression with a snapshot test.
- **Harden OSC embedding**: Escape control characters in the strings
  `jp_term::osc` splices into `set_title` and `hyperlink` sequences,
  unconditional and independent of the sanitize config knob.
- **Add the style.sanitize config knob**: Introduce `style.sanitize` in
  `jp_config` (`strip` / `visualize` / `off`), defaulting to `strip`.
- **Wire sanitization into chat rendering**: Apply `ContentWriter` to
  `render_request` (SGR kept, span-closed), `render_content`/`render_reasoning`
  (SGR dropped, no span), and add the decoded-text filter in `jp_md::render`,
  with snapshot tests covering escape-laden fixtures including entity-decoded
  escapes.
- **Audit and sanitize remaining render paths**: Wire sanitization into
  tool-result, history re-render, and table/list output (`jp conversation ls`
  titles, `jp conversation grep` hits), ensuring width-budgeted truncation
  always runs on escape-free text and JP-authored chrome stays unsanitized.
- **Enforce plain-text tool prompts**: Document and strip control characters
  from `jp_tool::Question` text, pre-amble, select options, and text-answer
  defaults in `ToolPrompter`, while preserving the original (unstripped) values
  returned to the tool.
  Independent of the config knob.
- **Document sanitization behavior**: Update `docs/configuration.md`, the README
  security section, and the ubiquitous-language glossary to describe the new
  `style.sanitize` knob and disambiguate display sanitization from storage
  sanitization and stream repair.

[RFD 096]: ../rfd/096-terminal-output-sanitization-for-untrusted-content.md
