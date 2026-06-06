import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import './index.css'
import App from './App.tsx'
import { ScreenshotOverlay } from './components/ScreenshotOverlay.tsx'
import { initializeTheme } from './theme'

initializeTheme()

const Root = window.location.hash === '#/screenshot-overlay' ? ScreenshotOverlay : App

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <Root />
  </StrictMode>,
)
