import DefaultTheme from 'vitepress/theme'
import Layout from './Layout.vue'
import NestBrunoLogo from './NestBrunoLogo.vue'
import AgentSkillInstall from './AgentSkillInstall.vue'
import './custom.css'

export default {
  extends: DefaultTheme,
  Layout,
  enhanceApp({ app }) {
    app.component('NestBrunoLogo', NestBrunoLogo)
    app.component('AgentSkillInstall', AgentSkillInstall)
  }
}
