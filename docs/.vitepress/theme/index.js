import DefaultTheme from 'vitepress/theme'
import { h } from 'vue'

import { docBefore } from '../jp-vite/theme/index.mjs'
import PageActions from './PageActions.vue'
import './custom.css'

export default {
    extends: DefaultTheme,
    Layout() {
        return h(DefaultTheme.Layout, null, {
            'doc-before': () => [h(PageActions), ...docBefore()],
        })
    },
}
