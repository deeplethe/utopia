import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import { BrowserRouter } from 'react-router-dom'
import { ThemeProvider } from 'next-themes'
import './index.css'
import App from './App.tsx'
import { AuthProvider } from '@/lib/auth'
import { Toaster } from '@/components/ui/sonner'

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    {/* attribute="class" toggles the `.dark` class our index.css theme keys off of. */}
    <ThemeProvider attribute="class" defaultTheme="system" enableSystem disableTransitionOnChange>
      <BrowserRouter>
        <AuthProvider>
          <App />
        </AuthProvider>
        <Toaster richColors position="top-right" />
      </BrowserRouter>
    </ThemeProvider>
  </StrictMode>,
)
