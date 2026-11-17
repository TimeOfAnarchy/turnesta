import type { FormEvent } from 'react';
import { useState } from 'react';
import { validateName } from '../api';
import type { BookingCommand, Preset, Specialist } from '../bookings';
import type { SaveProps } from './editorShared';
export type CatalogEditorProps = SaveProps & {
  kind: 'specialist' | 'preset';
  initial: Specialist | Preset | null;
  current?: Specialist | Preset;
};

export function useCatalogEditor({
  kind,
  initial,
  current,
  busy,
  journal,
  run,
  close,
}: CatalogEditorProps) {
  const [id] = useState(() => initial?.id ?? crypto.randomUUID());
  const [name, setName] = useState(initial?.name ?? '');
  const [description, setDescription] = useState(
    initial && 'description' in initial ? initial.description : '',
  );
  const [minutes, setMinutes] = useState(
    initial &&
      'duration_minutes' in initial &&
      initial.duration_minutes !== null
      ? String(initial.duration_minutes)
      : '',
  );
  const [enabled, setEnabled] = useState(initial?.enabled ?? true);
  const [version, setVersion] = useState(initial?.version ?? 0);
  const [error, setError] = useState<unknown>(null);
  const [invalid, setInvalid] = useState<string>();
  const conflict = Boolean(current && current.version !== version);
  async function save(command?: BookingCommand) {
    try {
      await run(command);
      close();
    } catch (e) {
      setError(e);
    }
  }
  function submit(event: FormEvent) {
    event.preventDefault();
    if (busy || journal.pending || conflict) return;
    const invalid =
      validateName(name) ||
      (kind === 'specialist' &&
      ([...description.trim()].length > 500 ||
        /\p{Cc}/u.test(description))
        ? 'Описание — до 500 символов, без переносов строк.'
        : undefined) ||
      (kind === 'preset' &&
      minutes !== '' &&
      (!/^\d+$/.test(minutes) || Number(minutes) < 1 || Number(minutes) > 1440)
        ? 'Длительность — от 1 до 1440 минут, либо оставьте поле пустым.'
        : undefined);
    setInvalid(invalid);
    if (invalid) return;
    void save(
      kind === 'specialist'
        ? {
            type: 'save_specialist',
            id,
            name,
            description,
            enabled,
            expected_version: version,
          }
        : {
            type: 'save_preset',
            id,
            name,
            duration_minutes: minutes === '' ? null : Number(minutes),
            enabled,
            expected_version: version,
          },
    );
  }
  return {
    name,
    setName,
    description,
    setDescription,
    minutes,
    setMinutes,
    enabled,
    setEnabled,
    setVersion,
    error,
    setError,
    invalid,
    conflict,
    save,
    submit,
  };
}
