import { useEffect, useRef, useState } from 'react';
import type { FormEvent } from 'react';
import { ApiError, errorMessage } from './api';
import { appointmentApi } from './appointments';
import type { Appointment } from './appointments';
import { Modal } from './components';

export function ClientExportDialog({ tenant, location, item, back, close }: {
  tenant: string; location: string; item: Appointment; back: () => void; close: () => void;
}) {
  const [contact, setContact] = useState<'phone' | 'email' | ''>(
    item.phone && item.email ? '' : item.phone ? 'phone' : 'email',
  );
  const [format, setFormat] = useState<'csv' | 'json'>('csv');
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState('');
  const [error, setError] = useState('');
  const active = useRef<AbortController | null>(null);
  useEffect(() => () => active.current?.abort(), []);

  async function download(event: FormEvent) {
    event.preventDefault();
    if (!contact || active.current) return;
    const controller = new AbortController();
    active.current = controller;
    setBusy(true); setError(''); setMessage('');
    try {
      const file = await appointmentApi.export(tenant, location, item.id, contact, format, item.version,
        AbortSignal.any([controller.signal, AbortSignal.timeout(15_000)]));
      if (controller.signal.aborted) return;
      const url = URL.createObjectURL(file);
      const link = document.createElement('a');
      link.href = url;
      link.download = `turnesta-client.${format}`;
      document.body.append(link);
      link.click();
      link.remove();
      window.setTimeout(() => URL.revokeObjectURL(url), 1000);
      setMessage(`Файл ${link.download} передан браузеру.`);
    } catch (e) {
      if (!controller.signal.aborted) setError(e instanceof ApiError && e.code === 'version_conflict'
        ? 'Карточка изменилась. Закройте её и откройте снова, чтобы проверить контакт.' : errorMessage(e));
    } finally {
      if (!controller.signal.aborted) { active.current = null; setBusy(false); }
    }
  }
  return <Modal icon="users" title="Выгрузка записей клиента" close={close}
    description="В файл войдут действующие и отменённые записи во всех точках организации с выбранным телефоном или email.">
    <form className="booking-form" onSubmit={download}>
      <fieldset disabled={busy} onChange={() => { setMessage(''); setError(''); }}>
        <div className="form-field">
          <label htmlFor="export-contact">Искать по контакту</label>
          <select id="export-contact" required value={contact} onChange={e => setContact(e.target.value as typeof contact)}>
            {!contact && <option value="">Выберите телефон или email</option>}
            {item.phone && <option value="phone">Телефон · {item.phone}</option>}
            {item.email && <option value="email">Email · {item.email}</option>}
          </select>
          <p className="field-hint">Если этим контактом пользуются несколько человек, в файл попадут записи каждого из них.</p>
        </div>
        <div className="form-field">
          <label htmlFor="export-format">Формат файла</label>
          <select id="export-format" value={format} onChange={e => setFormat(e.target.value as typeof format)}>
            <option value="csv">CSV — для таблицы</option>
            <option value="json">JSON — структурированные данные</option>
          </select>
        </div>
      </fieldset>
      {error && <p className="conflict-notice" role="alert">{error}</p>}
      {message && <p role="status">{message}</p>}
      <div className="dialog-actions">
        <button className="button secondary" type="button" onClick={back}>Назад к записи</button>
        <button className="button primary" disabled={busy || !contact}>{busy ? 'Готовим файл…' : 'Скачать файл'}</button>
      </div>
    </form>
  </Modal>;
}
