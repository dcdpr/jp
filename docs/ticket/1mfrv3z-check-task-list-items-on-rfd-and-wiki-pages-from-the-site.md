# Check task-list items on RFD and wiki pages from the site

- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-11-17
- **Label**: domain=tooling
- **Label**: type=follow-up

When serving the site with `jp vite serve`, checklist items in a ticket's
description can be checked and unchecked from the ticket page.
Task-list items on RFD pages (such as phases in an Implementation Plan, like RFD
072's) and on wiki pages still render as disabled checkboxes.

Because the site writes every document through `jp`, both `jp rfd` and `jp wiki`
need a flag to check and uncheck task-list items before the site can support
toggling them.

How the ticket implementation works:

- `tasklist.mjs` writes the item's lookup text to `data-checklist-item` on its
  checkbox.
  `config.mjs` (`ticketChecklistItems`) populates this attribute only for items
  in a ticket's description (excluding comments and embeds), reading the lookup
  text from the raw source line the same way the `ticket` crate's `checklist`
  module does.
- `theme/TicketChecklist.vue` enables those checkboxes and posts `check` or
  `uncheck` requests to `/__ticket`.
  `middleware.mjs` (`ticketCommands`) translates the request into `jp ticket
  edit <id> --check=<text>` (or `--uncheck=<text>`).

Items are matched by their text, so duplicate item text on the same page is
rejected as ambiguous.
When a request is rejected, the page displays the error and reverts the
checkbox.

## Checklist

- [ ] Add flags to check and uncheck an item by its text in `jp wiki edit` and
      `jp rfd`, reusing `ticket::checklist::mark` rather than duplicating the
      parser
- [ ] Populate `data-checklist-item` on RFD and wiki pages in the renderer, with
      the same exclusions (embedded items stay disabled because they belong to
      their source document)
- [ ] Generalize `TicketChecklist.vue` and the dev-server endpoints to support
      all three document kinds
