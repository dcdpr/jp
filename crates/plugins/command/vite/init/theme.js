import DefaultTheme from 'vitepress/theme'
import { h } from 'vue'

import { docBefore, enhanceApp } from '../jp-vite/theme/index.mjs'

// The default theme, with what the RFD and ticket pages need: the components
// they render their lists with (`<RfdIndex />` and the rest), and above a
// document, the trail back to the index, the references, and the status badge.
export default {
    extends: DefaultTheme,
    enhanceApp,
    Layout() {
        return h(DefaultTheme.Layout, null, {
            'doc-before': docBefore,
        })
    },
}
