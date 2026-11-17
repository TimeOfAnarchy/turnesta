import { useRef, useState } from 'react';

export function ConfirmationNumber({ number }: { number: string }) {
  const input = useRef<HTMLInputElement>(null);
  const [notice, setNotice] = useState('');
  async function copy() {
    try {
      await navigator.clipboard.writeText(number);
      setNotice('Номер скопирован.');
    } catch {
      input.current?.focus();
      input.current?.select();
      setNotice('Номер выделен. Скопируйте его через меню устройства.');
    }
  }
  return <div className="confirmation-number">
    <label htmlFor="appointment-number">Номер записи</label>
    <div className="confirmation-number-actions">
      <input id="appointment-number" ref={input} value={number} readOnly />
      <button type="button" className="button secondary" onClick={() => void copy()}>Копировать номер</button>
    </div>
    <p className="field-hint" role="status">{notice || 'Номер для связи с посетителем.'}</p>
  </div>;
}
