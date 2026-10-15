import { h } from 'vue'

import RfdBreadcrumb from './RfdBreadcrumb.vue'
import RfdLinkColors from './RfdLinkColors.vue'
import RfdReferences from './RfdReferences.vue'
import RfdStatusBadge from './RfdStatusBadge.vue'
import SiteProblem from './SiteProblem.vue'
import './style.css'

// What the RFD and ticket pages add above a document, for the project's theme
// entry to render in the default layout's `doc-before` slot:
//
//     import DefaultTheme from 'vitepress/theme'
//     import { h } from 'vue'
//     import { docBefore } from '../jp-vite/theme/index.mjs'
//
//     export default {
//         extends: DefaultTheme,
//         Layout: () => h(DefaultTheme.Layout, null, { 'doc-before': docBefore }),
//     }
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
