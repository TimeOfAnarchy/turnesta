import { useInfiniteQuery, useQuery, useQueryClient } from '@tanstack/react-query';
import { useEffect, useState } from 'react';
import { useWorkspace } from '../App';
import { useDayAppointments } from '../AppointmentEditors';
import { ApiError } from '../api';
import type { AppointmentSummary } from '../appointments';
import { isAppointmentAction } from '../appointments';
import type { BookingCommand, BookingReceipt, Offer, Preset, Specialist } from '../bookings';
import { bookingApi, BookingCommands, shiftDate, todayAt } from '../bookings';
import { keys } from '../queries';
import { distinct } from './collections';
const bookingKey = (tenant: string, location: string) =>
  ['organization', tenant, 'bookings', location] as const;
function storage() {
  try {
    return window.sessionStorage;
  } catch {
    return null;
  }
}
type Editor =
  | { kind: 'batch'; specialist: Specialist }
  | { kind: 'record'; offer: Offer; specialist: Specialist }
  | { kind: 'appointment'; id: string; erased: boolean; opening: string }
  | { kind: 'specialist'; item: Specialist | null }
  | { kind: 'preset'; item: Preset | null; back?: Specialist }
  | {
    kind: 'offer';
    item: Offer | null;
    copy?: boolean;
    specialist: Specialist;
  };
export type BookingScheduleProps = {
  tenant: string;
  location: string;
  zone: string;
  view: 'schedule' | 'catalog';
};

export function useBookingDay({
  tenant,
  location,
  zone,
  view,
}: BookingScheduleProps) {
  const { canManage, userId, role } = useWorkspace();
  const canBook = role !== 'employee';
  const client = useQueryClient();
  const base = bookingKey(tenant, location);
  const profiles = useInfiniteQuery({
    queryKey: [...base, 'specialists'],
    initialPageParam: null as string | null,
    queryFn: ({ pageParam, signal }) =>
      bookingApi.specialists(tenant, location, pageParam, signal),
    getNextPageParam: (p) => p.next_cursor ?? undefined,
    retry: false,
    refetchInterval: 10_000,
  });
  const presets = useInfiniteQuery({
    queryKey: [...base, 'presets'],
    initialPageParam: null as string | null,
    queryFn: ({ pageParam, signal }) =>
      bookingApi.presets(tenant, location, pageParam, signal),
    getNextPageParam: (p) => p.next_cursor ?? undefined,
    retry: false,
    refetchInterval: 10_000,
  });
  const specialists = distinct(
    profiles.data?.pages.flatMap((p) => p.items) ?? [],
  );
  const templates = distinct(presets.data?.pages.flatMap((p) => p.items) ?? []);
  const [chosen, setChosen] = useState('');
  const selected = specialists.find(
    (i) =>
      i.id ===
      (chosen || specialists.find((s) => s.enabled)?.id || specialists[0]?.id),
  );
  if (!chosen && selected) setChosen(selected.id);
  const { fetchNextPage: fetchMoreProfiles } = profiles;
  useEffect(() => {
    if (
      chosen &&
      !specialists.some((s) => s.id === chosen) &&
      profiles.hasNextPage &&
      !profiles.isFetching &&
      !profiles.isError
    )
      void fetchMoreProfiles();
  }, [
    chosen,
    specialists,
    profiles.hasNextPage,
    profiles.isFetching,
    profiles.isError,
    fetchMoreProfiles,
  ]);
  const today = todayAt(zone);
  const [date, setDate] = useState(today);
  const [withdrawn, setWithdrawn] = useState(false);
  const [cancelled, setCancelled] = useState(false);
  const offers = useInfiniteQuery({
    queryKey: [...base, 'offers', selected?.id, date, withdrawn],
    initialPageParam: null as string | null,
    enabled: Boolean(selected) && view === 'schedule',
    queryFn: ({ pageParam, signal }) =>
      bookingApi.offers(
        tenant,
        location,
        selected!.id,
        date,
        withdrawn,
        pageParam,
        signal,
      ),
    getNextPageParam: (p) => p.next_cursor ?? undefined,
    retry: false,
    refetchInterval: 10_000,
  });
  const showClients = canBook && Boolean(selected) && view === 'schedule';
  const appointments = useDayAppointments(tenant, location, selected?.id ?? '', date, false, showClients);
  const history = useDayAppointments(tenant, location, selected?.id ?? '', date, true, showClients && cancelled);
  const items = distinct(offers.data?.pages.flatMap((p) => p.items) ?? []);
  const nextDay = useQuery({
    queryKey: [...base, 'next-day', selected?.id, date],
    enabled:
      Boolean(selected) &&
      view === 'schedule' &&
      offers.isSuccess &&
      !items.length &&
      !withdrawn,
    staleTime: 60_000,
    retry: false,
    queryFn: async ({ signal }) => {
      for (let step = 1; step <= 14; step++) {
        const day = shiftDate(date, step);
        const page = await bookingApi.offers(
          tenant,
          location,
          selected!.id,
          day,
          false,
          null,
          signal,
        );
        const ready = page.items.filter((i) => i.state === 'ready');
        if (ready.length)
          return {
            date: day,
            count: ready.length,
            booked: ready.reduce(
              (sum, i) => sum + Math.min(i.reserved_places ?? 0, i.places),
              0,
            ),
          };
      }
      return null;
    },
  });
  const timed = items
    .filter((i) => i.mode !== 'by_agreement')
    .sort(
      (a, b) =>
        (a.starts_local ?? '').localeCompare(b.starts_local ?? '') ||
        a.id.localeCompare(b.id),
    );
  const agreement = items
    .filter((i) => i.mode === 'by_agreement')
    .sort(
      (a, b) =>
        Number(a.date === null) - Number(b.date === null) ||
        a.id.localeCompare(b.id),
    );
  const visible = new Set(items.map((i) => i.id));
  const clients = new Map<string, AppointmentSummary[]>();
  const placed = new Set<string>();
  const dayAppointments = appointments.data?.pages.flatMap((p) => p.items) ?? [];
  for (const a of dayAppointments) {
    if (a.status !== 'confirmed' || !visible.has(a.offer_id) || placed.has(a.id))
      continue;
    placed.add(a.id);
    clients.set(a.offer_id, [...(clients.get(a.offer_id) ?? []), a]);
  }
  const ready = items.filter((i) => i.state === 'ready');
  const booked = ready.reduce(
    (sum, i) => sum + Math.min(i.reserved_places ?? 0, i.places),
    0,
  );
  const free = ready.reduce(
    (sum, i) =>
      sum +
      (i.reserved_places === null || i.blocked_by_booking
        ? 0
        : Math.max(i.places - i.reserved_places, 0)),
    0,
  );
  const [journal] = useState(
    () => new BookingCommands({ tenant, location, user: userId }, storage()),
  );
  const [busy, setBusy] = useState(false);
  const [editor, setEditor] = useState<Editor | null>(null);
  let currentAppointment: AppointmentSummary | undefined;
  if (editor?.kind === 'appointment' && showClients && !appointments.isError && !(cancelled && history.isError)) {
    currentAppointment = dayAppointments.find((a) => a.id === editor.id);
    if (cancelled && !currentAppointment?.anonymized_at) {
      const historyRow = history.data?.pages.flatMap((p) => p.items).find((a) => a.id === editor.id);
      currentAppointment = historyRow ?? currentAppointment;
    }
  }
  if (
    editor &&
    ((['record', 'appointment'].includes(editor.kind) && !canBook) ||
      (!['record', 'appointment'].includes(editor.kind) && !canManage))
  )
    setEditor(null);
  const [success, setSuccess] = useState('');
  const [pageError, setPageError] = useState<unknown>(null);
  const [, render] = useState(0);
  const [online, setOnline] = useState(navigator.onLine);
  useEffect(() => {
    const changed = () => setOnline(navigator.onLine);
    window.addEventListener('online', changed);
    window.addEventListener('offline', changed);
    return () => {
      window.removeEventListener('online', changed);
      window.removeEventListener('offline', changed);
    };
  }, []);
  const disabled =
    busy || !online || Boolean(journal.storageError) || !canManage;
  const bookingDisabled =
    busy ||
    !online ||
    Boolean(journal.storageError) ||
    !canBook ||
    Boolean(journal.pending);
  async function run(command?: BookingCommand): Promise<BookingReceipt> {
    const intended = command ?? journal.pending?.command;
    if (
      !canBook ||
      (!canManage &&
        (!isAppointmentAction(intended ?? {}) || intended?.type === 'anonymize_appointment'))
    )
      throw new ApiError(403, 'forbidden');
    if (!navigator.onLine) throw new ApiError(0, 'offline');
    setBusy(true);
    setPageError(null);
    setSuccess('');
    try {
      const receipt = await journal.run(command);
      if (receipt.resource.kind === 'batch') {
        setChosen(receipt.resource.item.specialist_id);
        if (receipt.resource.item.dates[0])
          setDate(receipt.resource.item.dates[0]);
      }
      if (
        receipt.resource.kind === 'offer' ||
        receipt.resource.kind === 'appointment'
      ) {
        const item = receipt.resource.item;
        setChosen(item.specialist_id);
        if (item.date) setDate(item.date);
        if (
          receipt.resource.kind === 'offer' &&
          receipt.resource.item.state === 'withdrawn'
        )
          setWithdrawn(true);
      } else if (
        receipt.resource.kind === 'specialist' &&
        command?.type === 'save_specialist' &&
        command.expected_version === 0
      )
        setChosen(receipt.resource.item.id);
      setSuccess(
        receipt.resource.kind === 'batch'
          ? `Добавлено талонов: ${receipt.resource.item.offer_ids.length}. Клиентские записи не копировались.`
          : intended?.type === 'anonymize_appointment'
            ? 'Данные клиента удалены.'
            : receipt.resource.kind === 'appointment'
              ? receipt.resource.item.status === 'cancelled'
                ? 'Запись отменена. Место освобождено.'
                : 'Запись клиента сохранена.'
              : receipt.resource.kind === 'offer'
                ? 'Талон сохранён в расписании.'
                : receipt.resource.kind === 'preset'
                  ? 'Заготовка сохранена. Ранее созданные талоны не изменились.'
                  : 'Профиль специалиста сохранён.',
      );
      return receipt;
    } finally {
      try {
        await Promise.all([
          client.invalidateQueries({ queryKey: base }),
          client.invalidateQueries({ queryKey: keys.locations(tenant) }),
        ]);
      } finally {
        setBusy(false);
        render((n) => n + 1);
      }
    }
  }
  function open(value: Editor) {
    setSuccess('');
    setPageError(null);
    setEditor(value);
  }
  function openOffer(item: Offer | null, copy = false) {
    if (selected) open({ kind: 'offer', item, copy, specialist: selected });
  }
  function openAppointment(item: AppointmentSummary) {
    open({ kind: 'appointment', id: item.id, erased: Boolean(item.anonymized_at), opening: crypto.randomUUID() });
  }
  const actionsDisabled = disabled || Boolean(journal.pending);
  return {
    tenant,
    location,
    zone,
    view,
    canManage,
    role,
    canBook,
    profiles,
    presets,
    specialists,
    templates,
    setChosen,
    selected,
    today,
    date,
    setDate,
    withdrawn,
    setWithdrawn,
    cancelled,
    setCancelled,
    offers,
    appointments,
    history,
    items,
    nextDay,
    timed,
    agreement,
    clients,
    placed,
    ready,
    booked,
    free,
    journal,
    busy,
    editor,
    setEditor,
    currentAppointment,
    success,
    pageError,
    setPageError,
    online,
    bookingDisabled,
    run,
    open,
    openOffer,
    openAppointment,
    actionsDisabled,
  };
}
export type BookingDay = ReturnType<typeof useBookingDay>;
