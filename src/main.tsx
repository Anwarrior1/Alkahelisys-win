import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import App from './App';
import './styles.css';

const isPackagedWindowsDesktop = navigator.userAgent.includes('Windows')
  && ('__TAURI_INTERNALS__' in window || '__TAURI__' in window);

if (isPackagedWindowsDesktop) {
  document.documentElement.dataset.platform = 'windows-desktop';
}

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
