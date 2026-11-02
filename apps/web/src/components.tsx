import { useEffect, useRef } from 'react';
import type { ReactNode } from 'react';
import { errorMessage } from './api';

type IconName = 'calendar' | 'users' | 'clock' | 'locations' | 'settings' | 'grid' | 'arrow' | 'plus' | 'edit' | 'exit' | 'chevron' | 'check' | 'close' | 'globe' | 'heart' | 'refresh' | 'external' | 'queue' | 'left' | 'right';
export function Icon({ name, className = '' }: { name: IconName; className?: string }) {
  const paths: Record<IconName, ReactNode> = {
    calendar: <><rect x="3" y="5" width="18" height="16" rx="3"/><path d="M7 3v4m10-4v4M3 11h18m-13 5h1m6 0h1"/></>,
    users: <><circle cx="9" cy="7" r="3"/><path d="M3 21v-3a6 6 0 0 1 12 0v3m2-17a3 3 0 0 1 0 6m1 4a5 5 0 0 1 3 4v3"/></>,
    clock: <><circle cx="12" cy="12" r="9"/><path d="M12 6v6l4 2"/></>,
    queue: <><rect x="3" y="4" width="18" height="16" rx="3"/><path d="M7 9h2m4 0h4M7 15h2m4 0h4"/></>,
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
    heart: <path d="M20.8 4.6a5.5 5.5 0 0 0-7.8 0L12 5.7l-1.1-1.1a5.5 5.5 0 0 0-7.8 7.8L12 21l8.8-8.6a5.5 5.5 0 0 0 0-7.8Z"/>,
    refresh: <><path d="M20 7v5h-5M4 17v-5h5"/><path d="M6.2 6a8 8 0 0 1 13.5 5M4.3 13a8 8 0 0 0 13.5 5"/></>,
    external: <><path d="M14 3h7v7m0-7L10 14M10 3H4v17h17v-6"/></>,
    left: <path d="m14 6-6 6 6 6"/>,
    right: <path d="m10 6 6 6-6 6"/>,
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

export function Modal({ title, description, children, close, busy = false, icon = 'locations' }: {
  title: string; description: string; children: ReactNode; close: () => void; busy?: boolean; icon?: IconName;
}) {
  const dialog = useRef<HTMLDialogElement>(null);
  useEffect(() => {
    const element = dialog.current!;
    const previous = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    element.showModal();
    element.querySelector<HTMLElement>('input:not([disabled]), select:not([disabled]), textarea:not([disabled])')?.focus();
    return () => { element.close(); previous?.focus(); };
  }, []);
  return <dialog className="dialog" ref={dialog} aria-labelledby="dialog-title" aria-describedby="dialog-description" onCancel={(event) => {
    event.preventDefault();
    if (!busy) close();
  }}>
    <div className="dialog-heading"><div className="square-icon"><Icon name={icon}/></div><button type="button" className="icon-button" onClick={close} disabled={busy} aria-label="Закрыть"><Icon name="close"/></button></div>
    <h2 id="dialog-title">{title}</h2><p className="dialog-description" id="dialog-description">{description}</p>{children}
  </dialog>;
}
