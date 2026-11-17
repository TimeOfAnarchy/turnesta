import { useInfiniteQuery } from '@tanstack/react-query';
import type { FormEvent } from 'react';
import { useEffect, useRef, useState } from 'react';
import type { Appointment } from '../appointments';
import { appointmentApi, canAnonymizeAppointment } from '../appointments';
import type { BookingCommand, Offer } from '../bookings';
import { bookingApi, todayAt } from '../bookings';
import type { Controls } from './editorShared';
import { clientError, clientOf } from './editorShared';
export type AppointmentEditorProps = Controls & { tenant: string; location: string; initial: Appointment; role: string };

export function useAppointmentCommand({
  tenant,
  location,
  initial,
  role,
  busy,
  journal,
  run,
  close,
}: AppointmentEditorProps) {
  const [item, setItem] = useState(initial);
  const [client, setClient] = useState(() => clientOf(initial));
  const [mode, setMode] = useState<'view' | 'edit' | 'move' | 'cancel' | 'anonymize' | 'export'>('view');
  const backButton = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    if (mode === 'anonymize') backButton.current?.focus();
  }, [mode]);
  const [date, setDate] = useState(initial.date ?? todayAt(initial.time_zone));
  const [target, setTarget] = useState<Offer | null>(null);
  const [error, setError] = useState<unknown>(null);
  const [invalid, setInvalid] = useState<string>();
  const [loading, setLoading] = useState(false);
  const offers = useInfiniteQuery({
    queryKey: [
      'organization',
      tenant,
      'bookings',
      location,
      'move-offers',
      item.specialist_id,
      date,
    ],
    initialPageParam: null as string | null,
    enabled: mode === 'move',
    queryFn: ({ pageParam, signal }) =>
      bookingApi.offers(
        tenant,
        location,
        item.specialist_id,
        date,
        false,
        pageParam,
        signal,
      ),
    getNextPageParam: (p) => p.next_cursor ?? undefined,
    retry: false,
    refetchInterval: 10_000,
  });
  const options = [
    ...new Map(
      (offers.data?.pages.flatMap((p) => p.items) ?? [])
        .filter((o) => o.id !== item.offer_id && o.state === 'ready')
        .map((o) => [o.id, o]),
    ).values(),
  ].sort(
    (a, b) =>
      (a.starts_local ?? 'z').localeCompare(b.starts_local ?? 'z') ||
      a.id.localeCompare(b.id),
  );
  const freshTarget = target
    ? options.find((o) => o.id === target.id)
    : undefined;
  const staleTarget = Boolean(
    target && (!freshTarget || freshTarget.version !== target.version),
  );
  const blocked = busy || Boolean(journal.pending) || loading;
  const canErase = canAnonymizeAppointment(role, item);
  const canExport = role === 'owner' && !item.anonymized_at;
  function changeMode(value: typeof mode) {
    setMode(value);
    setError(null);
    setInvalid(undefined);
    setTarget(null);
  }
  async function refresh() {
    setLoading(true);
    try {
      const fresh = await appointmentApi.get(
        tenant,
        location,
        item.id,
        AbortSignal.timeout(10_000),
      );
      setItem(fresh);
      setClient(clientOf(fresh));
      setMode('view');
      setError(null);
    } catch (e) {
      setError(e);
    } finally {
      setLoading(false);
    }
  }
  async function save(command?: BookingCommand) {
    try {
      await run(command);
      close();
    } catch (e) {
      setError(e);
    }
  }
  function submit(e: FormEvent) {
    e.preventDefault();
    if (blocked || (item.status !== 'confirmed' && mode !== 'anonymize')) return;
    if (mode === 'anonymize' && canErase) {
      void save({ type: 'anonymize_appointment', id: item.id, specialist_id: item.specialist_id, expected_version: item.version });
      return;
    }
    if (mode === 'edit') {
      const err = clientError(client);
      setInvalid(err);
      if (!err)
        void save({
          type: 'update_client',
          id: item.id,
          specialist_id: item.specialist_id,
          expected_version: item.version,
          client,
        });
    }
    if (mode === 'move' && target && !staleTarget)
      void save({
        type: 'move_appointment',
        id: item.id,
        specialist_id: item.specialist_id,
        expected_version: item.version,
        offer_id: target.id,
        expected_offer_version: target.version,
      });
    if (mode === 'cancel')
      void save({
        type: 'cancel_appointment',
        id: item.id,
        specialist_id: item.specialist_id,
        expected_version: item.version,
      });
  }
  return {
    item,
    client,
    setClient,
    mode,
    backButton,
    date,
    setDate,
    target,
    setTarget,
    error,
    setError,
    invalid,
    setInvalid,
    loading,
    offers,
    options,
    staleTarget,
    blocked,
    canErase,
    canExport,
    changeMode,
    refresh,
    save,
    submit,
  };
}
