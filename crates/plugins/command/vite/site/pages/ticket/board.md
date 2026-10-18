---
aside: false
prev: false
next: false
---

# Ticket Board

Columns are stages of work; priority is the vertical order within a column.
Todo is therefore the work queue, read top-down, and it holds triaged and
untriaged work alike.
Done shows only the head of the column — the [full list] has the rest.

<TicketBoard />

Status lives in the ticket file; the order within a column lives in the board
file, `.board.json`, next to the tickets.
A ticket the board file doesn't mention sits at the bottom of its column, so a
newly filed ticket joins Todo below whatever has already been prioritised.

[full list]: ./
