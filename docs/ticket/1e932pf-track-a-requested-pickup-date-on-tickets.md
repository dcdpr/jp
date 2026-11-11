# Track a requested pickup date on tickets

- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-11-05
- **Label**: domain=plugins
- **Label**: type=enhancement

A ticket filed for pickup "in about two weeks" records that date only in its
description, as the `planner` persona writes it (T-1e92tft), and its rank drifts
later as tickets are inserted above it.
Nothing can answer "what is due this week?" or notice a ticket slipping past its
date.

One option is an open-valued label key in
`plugins.command.ticket.options.labels`, which needs no code and is filterable;
another is a metadata field the board can show.
Either turns a requested date into something the board shows as a commitment,
which is the decision to make before building it.
