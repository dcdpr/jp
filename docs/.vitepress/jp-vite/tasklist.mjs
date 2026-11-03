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

// A checkbox at the start of an item's text, with text after it.
const CHECKBOX = /^\[([ xX])\][ \t]+(?=\S)/

// What to take off the item's first text token: the checkbox and the space
// after it, which may be all the token holds.
const LEADING = /^\[[ xX]\][ \t]*/

// Turn the task-list items among markdown-it's block `tokens` into checkboxes.
// `Token` is markdown-it's token constructor.
export function renderTaskLists(tokens, Token) {
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
        box.content = `<input type="checkbox" class="task-list-item-checkbox" disabled${checked}> `

        first.content = first.content.replace(LEADING, '')
        const rest = first.content === '' ? inline.children.slice(1) : inline.children
        inline.children = [box, ...rest]

        const item = tokens[index - 2]
        addClass(item, 'task-list-item')
        const list = enclosingList(tokens, index - 2)
        if (list) addClass(list, 'contains-task-list')
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

// Add `name` to the classes `token` carries, once.
function addClass(token, name) {
    const existing = token.attrGet('class')
    if (existing?.split(/\s+/).includes(name)) return

    token.attrSet('class', existing ? `${existing} ${name}` : name)
}
