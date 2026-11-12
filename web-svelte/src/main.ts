import { mount } from 'svelte'
import App from './App.svelte'
import './styles/tokens.css'
import './styles/base.css'

const target = document.getElementById('app')
if (!target) throw new Error('index.html is missing the #app mount point')

const app = mount(App, {
  target,
})

export default app
