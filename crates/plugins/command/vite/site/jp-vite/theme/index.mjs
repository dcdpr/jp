import { defineAsyncComponent, h } from 'vue'

import RfdBreadcrumb from './RfdBreadcrumb.vue'
import RfdLinkColors from './RfdLinkColors.vue'
import RfdReferences from './RfdReferences.vue'
import RfdStatusBadge from './RfdStatusBadge.vue'
import SiteProblem from './SiteProblem.vue'
import './style.css'

// What the RFD and ticket pages need from the project's theme entry: the
// components the pages render their lists with, and what the default layout
// shows above a document.
//
//     import DefaultTheme from 'vitepress/theme'
//     import { h } from 'vue'
//     import { docBefore, enhanceApp } from '../jp-vite/theme/index.mjs'
//
//     export default {
//         extends: DefaultTheme,
//         enhanceApp,
//         Layout: () => h(DefaultTheme.Layout, null, { 'doc-before': docBefore }),
//     }

// Register the components a page renders a list with, by tag:
//
// - `<RfdIndex />`: the published RFDs.
// - `<RfdDraftIndex />`: the draft RFDs.
// - `<RfdBoard />`: the RFD priority board.
// - `<TicketIndex />`: every ticket.
// - `<TicketBoard />`: the ticket board.
//
// Each is loaded when a page first renders it, rather than with every page. A
// theme entry with an `enhanceApp` of its own calls this from it.
export function enhanceApp({ app }) {
    app.component('RfdIndex', defineAsyncComponent(() => import('./RfdIndex.vue')))
    app.component('RfdDraftIndex', defineAsyncComponent(() => import('./RfdDraftIndex.vue')))
    app.component('RfdBoard', defineAsyncComponent(() => import('./RfdBoard.vue')))
    app.component('TicketIndex', defineAsyncComponent(() => import('./TicketIndex.vue')))
    app.component('TicketBoard', defineAsyncComponent(() => import('./TicketBoard.vue')))
}

// What the RFD and ticket pages add above a document, for the default layout's
// `doc-before` slot.
//
// Each one renders only on the pages it concerns, so the slot can hold them on
// every page.
export function docBefore() {
    return [
        h(SiteProblem),
        h(RfdBreadcrumb),
        h(RfdReferences),
        h(RfdStatusBadge),
        h(RfdLinkColors),
    ]
}
