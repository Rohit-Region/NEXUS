import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import './assets/styles/globals.css';
import { App } from './App';
import { ErrorBoundary } from './components/ErrorBoundary/ErrorBoundary';

const rootElement = document.getElementById('root');
if (!rootElement) {
  throw new Error('Root element #root not found in document');
}

createRoot(rootElement).render(
  <StrictMode>
    <ErrorBoundary>
      <App />
    </ErrorBoundary>
  </StrictMode>,
);
