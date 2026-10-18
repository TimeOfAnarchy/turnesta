import { useEffect, useRef } from 'react';
import type { ReactNode } from 'react';
import { errorMessage } from './api';

type IconName = 'locations' | 'settings' | 'grid' | 'arrow' | 'plus' | 'edit' | 'exit' | 'chevron' | 'check' | 'close' | 'globe';
export function Icon({ name, className = '' }: { name: IconName; className?: string }) {
  const paths: Record<IconName, ReactNode> = {
    locations: <><path d="M20 10c0 6-8 11-8 11S4 16 4 10a8 8 0 1 1 16 0Z"/><circle cx="12" cy="10" r="2.5"/></>,
    settings: <><path d="M4 7h16M4 17h16"/><circle cx="9" cy="7" r="3"/><circle cx="15" cy="17" r="3"/></>,
    grid: <><rect x="3" y="3" width="7" height="7" rx="1.5"/><rect x="14" y="3" width="7" height="7" rx="1.5"/><rect x="3" y="14" width="7" height="7" rx="1.5"/><rect x="14" y="14" width="7" height="7" rx="1.5"/></>,
    arrow: <path d="M5 12h14m-6-6 6 6-6 6"/>,
    plus: <path d="M12 5v14M5 12h14"/>,
    edit: <><path d="m15 5 4 4M4 20l5-1L20 8a2.8 2.8 0 0 0-4-4L5 15l-1 5Z"/></>,
    exit: <><path d="M9 4H4v16h5m4-13 5 5-5 5m-5-5h13"/></>,
    chevron: <path d="m8 10 4 4 4-4"/>,
    check: <path d="m5 12 4 4L19 6"/>,
    close: <path d="m6 6 12 12M6 18 18 6"/>,
    globe: <><circle cx="12" cy="12" r="9"/><ellipse cx="12" cy="12" rx="4" ry="9"/><path d="M3 12h18"/></>,
  };
  return <svg className={`icon ${className}`} width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">{paths[name]}</svg>;
}

export function Brand() {
  return <span className="brand">turnesta<span aria-hidden="true">•</span></span>;
}

export function Loading({ label = 'Загружаем рабочее пространство…' }: { label?: string }) {
  return <div className="loading-state" role="status"><span className="spinner" aria-hidden="true"/><span>{label}</span></div>;
}

export function ErrorNotice({ error, retry }: { error: unknown; retry?: () => void }) {
  return <div className="error-notice" role="alert"><p>{errorMessage(error)}</p>{retry && <button className="text-button" type="button" onClick={retry}>Попробовать снова <span aria-hidden="true">↻</span></button>}</div>;
}

export function SuccessNotice({ children }: { children: ReactNode }) {
  return <p className="success-notice" role="status"><Icon name="check"/>{children}</p>;
}

export function Modal({ title, description, children, close, busy = false }: {
  title: string; description: string; children: ReactNode; close: () => void; busy?: boolean;
}) {
  const dialog = useRef<HTMLDialogElement>(null);
  useEffect(() => {
    const element = dialog.current!;
    const previous = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    element.showModal();
    return () => { element.close(); previous?.focus(); };
  }, []);
  return <dialog className="dialog" ref={dialog} aria-labelledby="dialog-title" aria-describedby="dialog-description" onCancel={(event) => {
    event.preventDefault();
    if (!busy) close();
  }}>
    <div className="dialog-heading"><div className="square-icon"><Icon name="locations"/></div><button type="button" className="icon-button" onClick={close} disabled={busy} aria-label="Закрыть"><Icon name="close"/></button></div>
    <h2 id="dialog-title">{title}</h2><p className="dialog-description" id="dialog-description">{description}</p>{children}
  </dialog>;
}
