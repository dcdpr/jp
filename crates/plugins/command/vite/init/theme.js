import DefaultTheme from 'vitepress/theme'
import { h } from 'vue'

import { docBefore } from '../jp-vite/theme/index.mjs'

// The default theme, with what the RFD and ticket pages add above a document:
// the trail back to the index, the references, and the status badge.
export default {
    extends: DefaultTheme,
    Layout() {
        return h(DefaultTheme.Layout, null, {
            'doc-before': docBefore,
        })
    },
}
