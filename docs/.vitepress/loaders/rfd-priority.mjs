// Pure parsing and validation for the priority board file
// (`docs/rfd/priority.json`). No Node imports: the Node-side loaders
// (`rfd-shared.mjs`) and the browser-side board (`RfdBoard.vue`) both consume
// this module, so it must run in either environment.

// Normalize a raw priority record into a fixed shape:
//
// - `planned`: milestone groups in board order. Each group's `ids` are the
//   RFDs sitting above that group's milestone marker; the last group's
//   milestone is `null` — the planned-but-unassigned block between the final
//   milestone marker and the unsorted cutoff. A trailing `null` group is
//   guaranteed.
// - `order`: the flattened planned ids, for rank arithmetic (the cutoff sits
//   at `order.length`).
// - `backlog` / `inDevelopment`: string id arrays.
//
// The legacy shape — a flat `order` array instead of `planned` — is accepted
// as a single unassigned group. Missing or malformed fields degrade to empty,
// matching "a missing file is an empty board".
export function normalizePriority(raw) {
    const ids = v => (Array.isArray(v) ? v.map(String) : [])

    let planned
    if (Array.isArray(raw?.planned)) {
        planned = raw.planned.map(g => ({
            milestone: g?.milestone == null ? null : String(g.milestone),
            ids: ids(g?.ids),
        }))
    } else {
        planned = [{ milestone: null, ids: ids(raw?.order) }]
    }
    if (planned.length === 0 || planned[planned.length - 1].milestone !== null) {
        planned.push({ milestone: null, ids: [] })
    }

    return {
        planned,
        order: planned.flatMap(g => g.ids),
        backlog: ids(raw?.backlog),
        inDevelopment: ids(raw?.in_development),
    }
}

// Reject structurally invalid milestone groups: duplicate names (markers are
// keyed by name), or an unassigned (`milestone: null`) group anywhere but
// last. The board UI never produces either; a hand-edit can.
export function checkMilestones(planned) {
    const seen = new Set()
    const dups = new Set()
    for (const group of planned) {
        if (group.milestone === null) continue
        if (seen.has(group.milestone)) dups.add(group.milestone)
        seen.add(group.milestone)
    }
    if (dups.size > 0) {
        const names = [...dups].sort().join(', ')
        return `Duplicate milestone names in priority board: ${names}.\n\n` +
            `Milestone markers are keyed by name; rename or merge the ` +
            `duplicates in \`docs/rfd/priority.json\`.`
    }

    const nullIndex = planned.findIndex(g => g.milestone === null)
    if (nullIndex !== planned.length - 1) {
        return `Misplaced unassigned group in priority board.\n\n` +
            `The \`"milestone": null\` group holds planned-but-unassigned ` +
            `RFDs and must be the last \`planned\` group in ` +
            `\`docs/rfd/priority.json\`.`
    }

    return null
}
