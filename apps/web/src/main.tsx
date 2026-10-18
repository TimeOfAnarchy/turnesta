import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { BrowserRouter } from 'react-router-dom';
import { ApiError, onSignedOut } from './api';
import { App } from './App';
import './styles.css';

const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      staleTime: 15_000,
      retry: (failures, error) => !(error instanceof ApiError && error.status < 500) && failures < 1,
    },
    mutations: { retry: false },
  },
});
let redirecting = false;
onSignedOut(() => {
  if (redirecting) return;
  redirecting = true;
  queryClient.clear();
  window.location.replace('/login');
});

createRoot(document.getElementById('root')!).render(
  <StrictMode><QueryClientProvider client={queryClient}><BrowserRouter basename="/app"><App/></BrowserRouter></QueryClientProvider></StrictMode>,
);
