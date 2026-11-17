import { useQueryClient } from '@tanstack/react-query';
import type { FormEvent } from 'react';
import { useEffect, useState } from 'react';
import { ApiError } from '../api';
import type { BookingCommand, ConflictNotice, Offer, OfferState, Preset, Specialist } from '../bookings';
import { bookingApi, conflictNotice } from '../bookings';
import { keys } from '../queries';
import type { SaveProps } from './editorShared';
type OfferDraft = {
  title: string;
  agreement: boolean;
  date: string;
  start: string;
  end: string;
  endDate: string;
  agreementDate: string;
  places: string;
  state: OfferState;
};
export function offerDraft(
  item: Offer | null,
  date: string,
  copy = false,
): OfferDraft {
  return {
    title: item?.title ?? '',
    agreement: item?.mode === 'by_agreement',
    date: item?.date ?? date,
    start: item?.starts_local?.slice(11, 16) ?? '',
    end: item?.ends_local?.slice(11, 16) ?? '',
    endDate: item?.ends_local?.slice(0, 10) ?? item?.date ?? date,
    agreementDate: item?.mode === 'by_agreement' ? (item.date ?? '') : date,
    places: String(item?.places ?? 1),
    state:
      copy && item?.state === 'withdrawn' ? 'draft' : (item?.state ?? 'ready'),
  };
}
export type OfferEditorProps = SaveProps & {
  tenant: string;
  location: string;
  initial: Offer | null;
  copy?: boolean;
  current?: Offer;
  specialist: Specialist;
  date: string;
  zone: string;
  presets: Preset[];
  morePresets?: () => void;
};

export function useOfferEditor({
  tenant,
  location,
  initial,
  copy,
  current,
  specialist,
  date,
  zone,
  presets,
  busy,
  journal,
  run,
  close,
}: OfferEditorProps) {
  const [id] = useState(() =>
    initial && !copy ? initial.id : crypto.randomUUID(),
  );
  const [draft, setDraft] = useState(() => ({
    ...offerDraft(initial, date, copy),
    ...(!initial && !specialist.enabled ? { state: 'draft' as const } : {}),
  }));
  const [version, setVersion] = useState(copy ? 0 : (initial?.version ?? 0));
  const [presetId, setPresetId] = useState('');
  const [autoDuration, setAutoDuration] = useState<number | null>(null);
  const [notice, setNotice] = useState<ConflictNotice | null>(null);
  const [error, setError] = useState<unknown>(null);
  const [invalid, setInvalid] = useState<string>();
  const conflict = Boolean(!copy && current && current.version !== version);
  const timezone = initial && !copy ? initial.time_zone : zone;
  const occupied = copy
    ? 0
    : (current?.reserved_places ?? initial?.reserved_places ?? 0);
  const client = useQueryClient();
  const [calculatedFor, setCalculatedFor] = useState<string | null>(null);
  const calculationKey =
    autoDuration !== null && !draft.agreement && draft.date && draft.start
      ? JSON.stringify([
          draft.date,
          draft.start,
          autoDuration,
          timezone,
          initial && !copy ? initial.id : null,
        ])
      : null;
  const calculationPending = Boolean(
    calculationKey && calculatedFor !== calculationKey,
  );
  const [calculation, setCalculation] = useState<{ key: string | null; error: unknown }>({
    key: calculationKey, error: null,
  });
  if (calculation.key !== calculationKey) setCalculation({ key: calculationKey, error: null });
  const calculationError = calculation.error;
  useEffect(() => {
    if (!calculationKey || autoDuration === null) return;
    const controller = new AbortController();
    let active = true;
    const timer = setTimeout(() => {
      void bookingApi
        .suggestTime(
          tenant,
          location,
          `${draft.date}T${draft.start}`,
          autoDuration,
          timezone,
          initial && !copy ? initial.id : null,
          controller.signal,
        )
        .then((result) => {
          if (!active) return;
          setDraft((old) => ({
            ...old,
            end: result.ends_local.slice(11, 16),
            endDate: result.ends_local.slice(0, 10),
          }));
          setNotice(null);
          setCalculatedFor(calculationKey);
          setCalculation({ key: calculationKey, error: null });
        })
        .catch((error) => {
          if (!active) return;
          setCalculation({ key: calculationKey, error });
          if (
            error instanceof ApiError &&
            error.code === 'booking_time_zone_changed'
          )
            void client.invalidateQueries({ queryKey: keys.locations(tenant) });
        });
    }, 200);
    return () => {
      active = false;
      clearTimeout(timer);
      controller.abort();
    };
  }, [
    calculationKey,
    autoDuration,
    draft.date,
    draft.start,
    timezone,
    initial,
    copy,
    tenant,
    location,
    client,
  ]);
  function update(values: Partial<OfferDraft>) {
    setNotice(null);
    setError(null);
    setInvalid(undefined);
    setDraft((old) => ({ ...old, ...values }));
  }
  function choosePreset(value: string) {
    const selected = presets.find((p) => p.id === value);
    const previous = presets.find((p) => p.id === presetId);
    setPresetId(value);
    setAutoDuration(selected?.duration_minutes ?? null);
    if (selected)
      update({
        ...(draft.title === '' || draft.title === previous?.name
          ? { title: selected.name }
          : {}),
      });
  }
  async function save(command?: BookingCommand) {
    try {
      await run(command);
      close();
    } catch (e) {
      setNotice(conflictNotice(e));
      setError(e);
    }
  }
  function submit(event: FormEvent) {
    event.preventDefault();
    if (busy || journal.pending || conflict || calculationPending) return;
    const start = `${draft.date}T${draft.start}`,
      end = draft.end ? `${draft.endDate}T${draft.end}` : null;
    const invalid =
      [...draft.title.trim()].length > 120 ||
      /\p{Cc}/u.test(draft.title)
        ? 'Название — до 120 символов, без управляющих знаков.'
        : draft.agreement
          ? !/^\d+$/.test(draft.places) ||
            Number(draft.places) < 1 ||
            Number(draft.places) > 1000
            ? 'Укажите от 1 до 1000 мест.'
            : undefined
          : !draft.date || !draft.start
            ? 'Укажите дату и время начала.'
            : end && (!draft.endDate || end <= start)
              ? 'Окончание должно быть позже начала. При необходимости измените дату окончания.'
              : undefined;
    setInvalid(invalid);
    if (invalid) return;
    void save({
      type: 'save_offer',
      id,
      specialist_id: initial && !copy ? initial.specialist_id : specialist.id,
      time_zone: timezone,
      title: draft.title,
      timing: draft.agreement
        ? {
            mode: 'by_agreement',
            date: draft.agreementDate || null,
            places: Number(draft.places),
          }
        : end
          ? { mode: 'interval', starts_local: start, ends_local: end }
          : { mode: 'start_only', starts_local: start },
      state: draft.state,
      expected_version: version,
      conflict_token: notice?.token ?? null,
    });
  }
  return {
    draft,
    setDraft,
    setVersion,
    presetId,
    setAutoDuration,
    notice,
    setNotice,
    error,
    setError,
    invalid,
    conflict,
    timezone,
    occupied,
    calculationPending,
    calculationError,
    update,
    choosePreset,
    save,
    submit,
  };
}
