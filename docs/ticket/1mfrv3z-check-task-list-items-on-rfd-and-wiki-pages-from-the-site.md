# Check task-list items on RFD and wiki pages from the site

- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-11-17
- **Label**: domain=tooling
- **Label**: type=follow-up

On a dev server `jp vite serve` started, a ticket's checklist items can be
checked and unchecked from the ticket's page.
Task-list items on RFD pages (an Implementation Plan's phases, such as RFD
072's) and on wiki pages still render as disabled checkboxes.

The site writes every document through `jp`, and neither `jp rfd` nor `jp wiki`
has a command that checks an item, so this needs one in each before the site can
offer it.

How the ticket version works, for reference:

- `tasklist.mjs` puts the item's text in `data-checklist-item` on its checkbox.
  `config.mjs` (`ticketChecklistItems`) decides which items get it: on ticket
  pages only, items in the description, not in comments or embeds, with the text
  read from the item's source line as the `ticket` crate's `checklist` module
  reads it.

- `theme/TicketChecklist.vue` enables those checkboxes and posts `check` or
  `uncheck` to `/__ticket`.
  `middleware.mjs` (`ticketCommands`) turns the request into `jp ticket edit
  <id> --check=<text>`.

- [ ] Add a way to check and uncheck an item by its text to `jp wiki edit` and
      to `jp rfd`, reusing the `ticket` crate's `checklist::mark` rather than a
      second implementation

- [ ] Give RFD and wiki pages their item text in the renderer, with the same
      exclusions (embedded items stay disabled, since they are checked on the
      page they come from)

- [ ] Generalize `TicketChecklist.vue` and the endpoints to the three document
      kinds

Items are found by their text, so two items with the same text are refused as
ambiguous on every kind of page.
The page shows the refusal and leaves the box as it was.
