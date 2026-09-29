import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import App from './App';
import './index.css';
import { initTheme } from './lib/theme';
import { initToken, startHeartbeat } from './lib/transport';

initToken();
startHeartbeat();
initTheme();

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
