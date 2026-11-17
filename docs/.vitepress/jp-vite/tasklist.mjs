// Render markdown task-list items, `- [ ] text` and `- [x] text`, as disabled
// checkboxes.
//
// A ticket's checklist and an RFD's Implementation Plan are written this way,
// and VitePress renders the brackets as text. The rule is the one `jp ticket`
// counts a checklist by (the `ticket` crate's `checklist` module): a checkbox
// opening the first paragraph of a list item, then whitespace and text.
//
// The list item gets `task-list-item` and its list `contains-task-list`, the
// classes GitHub uses, which `theme/style.css` styles.
//
// Items that `jp ticket edit --check` can toggle carry their lookup text in
// `data-checklist-item` (see `ticketItemText`), and `theme/TicketChecklist.vue`
// enables them when the dev server accepts edits. Checkboxes render disabled in
// HTML so static builds stay read-only.

import { isSectionSource } from './embed.mjs'
import { writtenSource } from './join-inline-code.mjs'
import { checklistItem, descriptionEnd } from './loaders/ticket-shared.mjs'

// A checkbox at the start of an item's text, with text after it.
const CHECKBOX = /^\[([ xX])\][ \t]+(?=\S)/

// What to take off the item's first text token: the checkbox and the space
// after it, which may be all the token holds.
const LEADING = /^\[[ xX]\][ \t]*/

// Turn the task-list items among markdown-it's block `tokens` into checkboxes.
// `Token` is markdown-it's token constructor.
//
// `itemText` receives the inline token of each item's first paragraph and
// returns the lookup text for `data-checklist-item`, or `null` if the item is
// not checkable.
export function renderTaskLists(tokens, Token, itemText = () => null) {
    for (let index = 2; index < tokens.length; index++) {
        const inline = tokens[index]
        if (inline.type !== 'inline') continue
        if (tokens[index - 1].type !== 'paragraph_open') continue
        if (tokens[index - 2].type !== 'list_item_open') continue

        const match = inline.content.match(CHECKBOX)
        const first = inline.children?.[0]
        if (!match || first?.type !== 'text' || !LEADING.test(first.content)) continue

        const box = new Token('html_inline', '', 0)
        const checked = match[1] === ' ' ? '' : ' checked'
        const text = itemText(inline)
        const attribute = text ? ` data-checklist-item="${escapeAttribute(text)}"` : ''
        box.content =
            `<input type="checkbox" class="task-list-item-checkbox" disabled${checked}${attribute}> `

        first.content = first.content.replace(LEADING, '')
        const rest = first.content === '' ? inline.children.slice(1) : inline.children
        inline.children = [box, ...rest]

        const item = tokens[index - 2]
        addClass(item, 'task-list-item')
        const list = enclosingList(tokens, index - 2)
        if (list) addClass(list, 'contains-task-list')
    }
}

// The `itemText` for the block tokens of a ticket's page, parsed from `src` in
// markdown-it's `env`: the text `jp ticket edit --check` finds each item of the
// ticket's description by.
//
// Items in comments have none, since the tooling never changes a comment. A
// document parsed for a section another page embeds gets no `itemText` at all:
// its tokens end up on that page, where the ticket they would be checked
// through is the embedding one.
//
// The text comes from the item's first line as written, as the `ticket` crate's
// `checklist` module reads it, rather than from `src`, where the join rule may
// have moved the next line of a wrapped code span onto it.
export function ticketItemText(env, src) {
    if (isSectionSource(env)) return undefined

    const lines = src.split('\n')
    const end = descriptionEnd(lines)
    const written = writtenSource(env, src)
    // The join rule replaces a newline with one space, so a line of `src`
    // starts at the same offset in `written`.
    const starts = []
    let offset = 0
    for (const line of lines) {
        starts.push(offset)
        offset += line.length + 1
    }

    return (inline) => {
        const line = inline.map?.[0]
        if (line === undefined || line >= end) return null

        const stop = written.indexOf('\n', starts[line])
        const first = written.slice(starts[line], stop < 0 ? undefined : stop)
        return checklistItem(first)?.text ?? null
    }
}

// The list opening around the list item at `at`.
function enclosingList(tokens, at) {
    const level = tokens[at].level - 1
    for (let index = at - 1; index >= 0; index--) {
        const token = tokens[index]
        if (token.level === level && token.nesting === 1 && token.type.endsWith('_list_open')) {
            return token
        }
    }

    return null
}

const ESCAPES = { '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' }

function escapeAttribute(text) {
    return text.replace(/[&<>"]/g, char => ESCAPES[char])
}

// Add `name` to the classes `token` carries, once.
function addClass(token, name) {
    const existing = token.attrGet('class')
    if (existing?.split(/\s+/).includes(name)) return

    token.attrSet('class', existing ? `${existing} ${name}` : name)
}
