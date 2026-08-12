import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import { BrowserRouter } from 'react-router-dom'
import { ThemeProvider } from 'next-themes'
import './index.css'
import App from './App.tsx'
import { AuthProvider } from '@/lib/auth'
import { Toaster } from '@/components/ui/sonner'
import { I18nProvider } from '@/lib/i18n'
import { ConfirmProvider } from '@/lib/confirm'

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    {/* attribute="class" toggles the `.dark` class our index.css theme keys off of. */}
    <ThemeProvider attribute="class" defaultTheme="system" enableSystem disableTransitionOnChange>
      <I18nProvider>
        <BrowserRouter>
          <AuthProvider>
            <ConfirmProvider>
              <App />
            </ConfirmProvider>
          </AuthProvider>
          <Toaster richColors position="top-right" />
        </BrowserRouter>
      </I18nProvider>
    </ThemeProvider>
  </StrictMode>,
)
