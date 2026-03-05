import DefaultTheme from 'vitepress/theme'
import RfdBreadcrumb from './RfdBreadcrumb.vue'
import { h } from 'vue'
import './custom.css'

export default {
    extends: DefaultTheme,
    Layout() {
        return h(DefaultTheme.Layout, null, {
            'doc-before': () => h(RfdBreadcrumb),
        })
    },
}
