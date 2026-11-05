import { useEffect, useRef, useState } from 'react';
import type { ReactNode } from 'react';
import { errorMessage } from './api';
import { time24Pattern, typedTime } from './time24';

type IconName = 'calendar' | 'users' | 'clock' | 'locations' | 'settings' | 'grid' | 'arrow' | 'plus' | 'edit' | 'exit' | 'chevron' | 'check' | 'close' | 'globe' | 'heart' | 'refresh' | 'external' | 'queue' | 'left' | 'right' | 'key' | 'help';
// One 24px grid, 1.75 stroke and round joins keep every glyph optically consistent.
export function Icon({ name, className = '' }: { name: IconName; className?: string }) {
  const paths: Record<IconName, ReactNode> = {
    calendar: <><rect x="3.5" y="4.5" width="17" height="16" rx="3"/><path d="M8 2.75v3.5M16 2.75v3.5M3.5 9.5h17"/></>,
    users: <><circle cx="9" cy="8" r="3.5"/><path d="M2.75 20a6.25 6.25 0 0 1 12.5 0M15.5 4.7a3.5 3.5 0 0 1 0 6.6M17.75 14.3A6.25 6.25 0 0 1 21.25 20"/></>,
    clock: <><circle cx="12" cy="12" r="8.75"/><path d="M12 7.5V12l3 2"/></>,
    queue: <><path d="M3.5 7.5a2 2 0 0 1 2-2h13a2 2 0 0 1 2 2V10a2 2 0 0 0 0 4v2.5a2 2 0 0 1-2 2h-13a2 2 0 0 1-2-2V14a2 2 0 0 0 0-4Z"/><path d="M9.5 5.5v2M9.5 11v2M9.5 16.5v2M13 10h4M13 14h2.5"/></>,
    locations: <><path d="M12 21.25s-6.75-5.5-6.75-11.25a6.75 6.75 0 0 1 13.5 0c0 5.75-6.75 11.25-6.75 11.25Z"/><circle cx="12" cy="10" r="2.5"/></>,
    settings: <><path d="M4 6.5h8.5M16.5 6.5H20M4 12h2.5M10.5 12H20M4 17.5h10M18 17.5h2"/><circle cx="14.5" cy="6.5" r="2"/><circle cx="8.5" cy="12" r="2"/><circle cx="16" cy="17.5" r="2"/></>,
    grid: <><rect x="3.5" y="3.5" width="7" height="7" rx="2"/><rect x="13.5" y="3.5" width="7" height="7" rx="2"/><rect x="3.5" y="13.5" width="7" height="7" rx="2"/><rect x="13.5" y="13.5" width="7" height="7" rx="2"/></>,
    arrow: <path d="M5 12h14M13 6l6 6-6 6"/>,
    help: <><circle cx="12" cy="12" r="8.75"/><path d="M9.5 9.5a2.5 2.5 0 1 1 3.7 2.2c-.8.45-1.2.95-1.2 1.8M12 16.75v.01"/></>,
    plus: <path d="M12 5v14M5 12h14"/>,
    edit: <path d="M14.5 5.5l4 4M4 20l1-4.5L16 4.5a2.1 2.1 0 0 1 3 0l.5.5a2.1 2.1 0 0 1 0 3L8.5 19Z"/>,
    exit: <path d="M9.5 20.5h-3a3 3 0 0 1-3-3v-11a3 3 0 0 1 3-3h3M15.5 16.5 20 12l-4.5-4.5M20 12H9"/>,
    chevron: <path d="m6.5 9.5 5.5 5.5 5.5-5.5"/>,
    check: <path d="m4.75 12.5 4.5 4.5 10-10"/>,
    close: <path d="M6.5 6.5l11 11M17.5 6.5l-11 11"/>,
    globe: <><circle cx="12" cy="12" r="8.75"/><path d="M3.25 12h17.5M12 3.25c2.4 2.4 3.5 5.4 3.5 8.75s-1.1 6.35-3.5 8.75c-2.4-2.4-3.5-5.4-3.5-8.75s1.1-6.35 3.5-8.75Z"/></>,
    heart: <path d="M12 20s-7.75-4.6-7.75-10.1A4.4 4.4 0 0 1 12 7.1a4.4 4.4 0 0 1 7.75 2.8C19.75 15.4 12 20 12 20Z"/>,
    refresh: <path d="M19.5 9.5A8 8 0 0 0 5 7.5M4.5 3.75V8h4.25M4.5 14.5a8 8 0 0 0 14.5 2M19.5 20.25V16h-4.25"/>,
    external: <path d="M13.5 4h6.5v6.5M20 4l-8.5 8.5M17.5 14v3.5a2.5 2.5 0 0 1-2.5 2.5H6.5A2.5 2.5 0 0 1 4 17.5V9a2.5 2.5 0 0 1 2.5-2.5H10"/>,
    left: <path d="m14.5 6-6 6 6 6"/>,
    right: <path d="m9.5 6 6 6-6 6"/>,
    key: <><circle cx="8" cy="15.5" r="4"/><path d="M10.9 12.6 19.5 4M16 7.5l2.5 2.5M13.5 10l2 2"/></>,
  };
  return <svg className={`icon ${className}`} width="20" height="20" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.75" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">{paths[name]}</svg>;
}

export function BrandMark({ className = '' }: { className?: string }) {
  return <svg className={`brand-mark ${className}`} width="28" height="28" viewBox="0 0 32 32" aria-hidden="true">
    <rect width="32" height="32" rx="9" fill="currentColor"/>
    <path d="M14 8.5v12.25a4 4 0 0 0 4 4h3.25M8.75 15.25c1.5-1.3 3.3-2 5.35-2h7.15" fill="none" stroke="#fff" strokeWidth="3.25" strokeLinecap="round" strokeLinejoin="round"/>
  </svg>;
}

export function Brand() {
  return <span className="brand"><BrandMark/><span className="brand-word">turnesta<span className="brand-dot" aria-hidden="true">.</span></span></span>;
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

// 24-hour HH:MM field: reports only complete times or '' and restores the last valid time on blur.
export function TimeInput({ value, onChange, ...field }: {
  id: string; value: string; onChange: (value: string) => void; disabled?: boolean; required?: boolean;
}) {
  const [text, setText] = useState(value);
  useEffect(() => setText(value), [value]);
  return <input {...field} type="text" inputMode="numeric" autoComplete="off" placeholder="ЧЧ:ММ" maxLength={5}
    pattern={time24Pattern} title="Время в формате ЧЧ:ММ, например 09:30" value={text}
    onChange={(e) => {
      const next = typedTime(e.target.value);
      setText(next.text);
      if (next.value !== null) onChange(next.value);
      else if (!next.text) onChange('');
    }}
    onBlur={() => { if (text && typedTime(text).value === null) setText(value); }}/>;
}

export function Modal({ title, description, children, close, busy = false, icon = 'locations' }: {
  title: string; description: string; children: ReactNode; close: () => void; busy?: boolean; icon?: IconName;
}) {
  const dialog = useRef<HTMLDialogElement>(null);
  useEffect(() => {
    const element = dialog.current!;
    const previous = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    element.showModal();
    [...element.querySelectorAll<HTMLElement>('input:not([disabled]), select:not([disabled]), textarea:not([disabled])')].find((field) => !field.closest('details:not([open])'))?.focus();
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
