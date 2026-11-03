import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { test } from 'node:test'

import { checklistItems, parseTicket } from '../site/jp-vite/loaders/ticket-shared.mjs'

// The cases the `ticket` crate's reader is tested against, so the site counts
// a checklist the way `jp ticket` does.
const CASES = JSON.parse(readFileSync(
    new URL('../../../../internal/ticket/src/checklist_cases.json', import.meta.url),
    'utf-8',
))

test('the shared cases read as the ticket crate reads them', () => {
    for (const { case: name, description, items } of CASES) {
        const read = checklistItems(description).map(item => [item.checked, item.text])
        assert.deepEqual(read, items, name)
    }
})

const TICKET = `# Exercise recorded tool rounds

- **Status**: Todo
- **Authors**: jp
- **Date**: 2026-08-21

- [x] Keep tools declared
- [ ] Cover a second call

## Comments

-----

- **From**: jp
- **Date**: 2026-09-21T19:55:44Z

Suggested:

- [ ] Carry the reasoning setting
`

// The tooling only ever appends comments, so an item proposed in one could
// never be checked through it, and isn't counted.
test('a ticket counts the checklist in its description only', () => {
    const ticket = parseTicket(TICKET, '063sw3q-exercise.md')

    assert.deepEqual(ticket.checklist, { checked: 1, count: 2 })
    assert.equal(ticket.comments, 1)
})

test('a ticket without a checklist carries none', () => {
    const ticket = parseTicket('# Plain\n\n- **Status**: Todo\n\nJust prose.\n', '0aaaaaa-plain.md')

    assert.equal(ticket.checklist, null)
})
