import { defineAsyncComponent, h } from 'vue'

import DocLabels from './DocLabels.vue'
import DocBreadcrumb from './DocBreadcrumb.vue'
import DocLinkColors from './DocLinkColors.vue'
import DocReferences from './DocReferences.vue'
import SiteProblem from './SiteProblem.vue'
import WikiEditor from './WikiEditor.vue'
import './style.css'

// What the RFD, ticket, and wiki pages need from the project's theme entry: the
// components the pages render their lists with, and what the default layout
// shows above and below a document.
//
//     import DefaultTheme from 'vitepress/theme'
//     import { h } from 'vue'
//     import { docAfter, docBefore, enhanceApp } from '../jp-vite/theme/index.mjs'
//
//     export default {
//         extends: DefaultTheme,
//         enhanceApp,
//         Layout: () => h(DefaultTheme.Layout, null, {
//             'doc-before': docBefore,
//             'doc-after': docAfter,
//         }),
//     }

// Register the components a page renders a list with, by tag:
//
// - `<RfdIndex />`: the published RFDs.
// - `<RfdDraftIndex />`: the draft RFDs.
// - `<RfdBoard />`: the RFD priority board.
// - `<TicketIndex />`: every ticket.
// - `<TicketBoard />`: the ticket board.
// - `<WikiIndex />`: every wiki page.
// - `<WikiChildren />`: the wiki pages under the page it is on, or with no page
//   around it, the top pages.
// - `<DocLabels />`: a document's labels, which every RFD, ticket, and wiki
//   page's header shows.
//
// Each but `<DocLabels />` is loaded when a page first renders it, rather than
// with every page; that one is on most of them. A theme entry with an
// `enhanceApp` of its own calls this from it.
export function enhanceApp({ app }) {
    app.component('DocLabels', DocLabels)
    app.component('RfdIndex', defineAsyncComponent(() => import('./RfdIndex.vue')))
    app.component('RfdDraftIndex', defineAsyncComponent(() => import('./RfdDraftIndex.vue')))
    app.component('RfdBoard', defineAsyncComponent(() => import('./RfdBoard.vue')))
    app.component('TicketIndex', defineAsyncComponent(() => import('./TicketIndex.vue')))
    app.component('TicketBoard', defineAsyncComponent(() => import('./TicketBoard.vue')))
    app.component('WikiIndex', defineAsyncComponent(() => import('./WikiIndex.vue')))
    app.component('WikiChildren', defineAsyncComponent(() => import('./WikiChildren.vue')))
}

// What the RFD, ticket, and wiki pages add above a document, for the default
// layout's `doc-before` slot: the problem `jp vite serve` found, if any, the
// trail back to the index, link colours, and on a wiki page, where it sits in
// the wiki, with an editor on a dev server that accepts edits.
//
// Each one renders only on the pages it concerns, so the slot can hold them on
// every page.
export function docBefore() {
    return [
        h(SiteProblem),
        h(DocBreadcrumb),
        h(DocLinkColors),
        h(WikiEditor),
    ]
}

// What the RFD, ticket, and wiki pages add below a document, for the default
// layout's `doc-after` slot: what the document references, and what references
// it.
export function docAfter() {
    return [h(DocReferences)]
}
