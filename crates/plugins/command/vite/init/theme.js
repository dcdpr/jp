import DefaultTheme from 'vitepress/theme'
import { h } from 'vue'

import { docAfter, docBefore, enhanceApp } from '../jp-vite/theme/index.mjs'

// The default theme, with what the RFD, ticket, and wiki pages need: the
// components they render their lists with (`<RfdIndex />` and the rest), above
// a document the trail back to the index, and below it the references.
export default {
    extends: DefaultTheme,
    enhanceApp,
    Layout() {
        return h(DefaultTheme.Layout, null, {
            'doc-before': docBefore,
            'doc-after': docAfter,
        })
    },
}
