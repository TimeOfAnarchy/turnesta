import { Component } from 'react';
import type { ReactNode } from 'react';
import { Brand } from './components';

export class ErrorBoundary extends Component<{ children: ReactNode }, { failed: boolean }> {
  state = { failed: false };

  static getDerivedStateFromError() {
    return { failed: true };
  }

  render() {
    if (this.state.failed) {
      return <main className="entry-state">
        <Brand/>
        <div className="error-fallback" role="alert">
          <h1>Что-то пошло не так</h1>
          <p>Обновите страницу, чтобы снова открыть кабинет.</p>
          <button className="button primary" type="button" onClick={() => window.location.reload()}>
            Обновить страницу
          </button>
        </div>
      </main>;
    }
    return this.props.children;
  }
}
