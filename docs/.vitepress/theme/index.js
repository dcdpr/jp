import DefaultTheme from 'vitepress/theme'
import { h } from 'vue'

import { docBefore, enhanceApp } from '../jp-vite/theme/index.mjs'
import PageActions from './PageActions.vue'
import './custom.css'

export default {
    extends: DefaultTheme,
    // Registers the components the RFD and ticket pages render their lists with.
    enhanceApp,
    Layout() {
        return h(DefaultTheme.Layout, null, {
            'doc-before': () => [h(PageActions), ...docBefore()],
        })
    },
}
