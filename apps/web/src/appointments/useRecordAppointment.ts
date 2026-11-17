import type { FormEvent } from 'react';
import { useState } from 'react';
import type { ClientInput } from '../appointments';
import type { BookingCommand, Offer, Specialist } from '../bookings';
import type { Controls } from './editorShared';
import { clientError, emptyClient } from './editorShared';
export type RecordAppointmentEditorProps = Controls & { offer: Offer; current?: Offer; specialist: Specialist };

export function useRecordAppointment({
  offer,
  current,
  specialist,
  busy,
  journal,
  run,
  close,
}: RecordAppointmentEditorProps) {
  const [id] = useState(() => crypto.randomUUID());
  const [client, setClient] = useState<ClientInput>(emptyClient);
  const [version, setVersion] = useState(offer.version);
  const [error, setError] = useState<unknown>(null);
  const [invalid, setInvalid] = useState<string>();
  const changed = Boolean(current && current.version !== version);
  const unavailable = Boolean(
    current &&
      (current.state !== 'ready' ||
        (current.reserved_places !== null &&
          current.reserved_places >= current.places) ||
        current.blocked_by_booking),
  );
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
    if (
      busy ||
      journal.pending ||
      changed ||
      unavailable ||
      !specialist.enabled
    )
      return;
    const err = clientError(client);
    setInvalid(err);
    if (!err)
      void save({
        type: 'record_appointment',
        id,
        specialist_id: specialist.id,
        offer_id: offer.id,
        expected_offer_version: version,
        client,
      });
  }
  return {
    client,
    setClient,
    setVersion,
    error,
    setError,
    invalid,
    setInvalid,
    changed,
    unavailable,
    save,
    submit,
  };
}
