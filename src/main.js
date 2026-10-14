import { createApp } from 'vue'
import { createPinia } from 'pinia'
import App from './App.vue'
import './styles/global.css'
import { installTauriBridge } from './api/tauriBridge'

installTauriBridge()
createApp(App).use(createPinia()).mount('#app')
