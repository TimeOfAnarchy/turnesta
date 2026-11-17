import { AppointmentDetails, RecordAppointmentEditor } from '../AppointmentEditors';
import { BookingBatchEditor } from '../BookingBatchEditor';
import { CatalogEditor, OfferEditor } from '../BookingEditors';
import { Modal } from '../components';
import type { BookingDay } from './useBookingDay';

export function BookingDialogs({
  tenant,
  location,
  zone,
  role,
  presets,
  specialists,
  templates,
  date,
  items,
  journal,
  busy,
  editor,
  setEditor,
  currentAppointment,
  run,
  open,
}: Pick<
  BookingDay,
  | 'tenant'
  | 'location'
  | 'zone'
  | 'role'
  | 'presets'
  | 'specialists'
  | 'templates'
  | 'date'
  | 'items'
  | 'journal'
  | 'busy'
  | 'editor'
  | 'setEditor'
  | 'currentAppointment'
  | 'run'
  | 'open'
>) {
  return (
    <>
      {editor &&
        (editor.kind === 'batch' ? (
          <BookingBatchEditor
            tenant={tenant}
            location={location}
            specialist={
              specialists.find((s) => s.id === editor.specialist.id) ??
              editor.specialist
            }
            zone={zone}
            date={date}
            presets={templates}
            morePresets={
              presets.hasNextPage
                ? () => void presets.fetchNextPage()
                : undefined
            }
            createPreset={() =>
              open({ kind: 'preset', item: null, back: editor.specialist })
            }
            sourceEmpty={!items.length}
            busy={busy}
            journal={journal}
            run={run}
            close={() => setEditor(null)}
          />
        ) : editor.kind === 'record' ? (
          <RecordAppointmentEditor
            key={`record:${editor.offer.id}`}
            offer={editor.offer}
            current={items.find((i) => i.id === editor.offer.id)}
            specialist={
              specialists.find((s) => s.id === editor.specialist.id) ??
              editor.specialist
            }
            busy={busy}
            journal={journal}
            run={run}
            close={() => setEditor(null)}
          />
        ) : editor.kind === 'appointment' ? (
          currentAppointment && Boolean(currentAppointment.anonymized_at) === editor.erased ? (
            <AppointmentDetails
              key={editor.opening}
              tenant={tenant}
              location={location}
              id={editor.id}
              opening={editor.opening}
              role={role}
              busy={busy}
              journal={journal}
              run={run}
              close={() => setEditor(null)}
            />
          ) : (
            <Modal title="Карточка обновлена" close={() => setEditor(null)}
              description={currentAppointment?.anonymized_at
                ? 'Данные клиента удалены. Открытый черновик закрыт.'
                : 'Запись сейчас недоступна. Проверьте расписание.'}>
              <button type="button" className="button secondary" onClick={() => setEditor(null)}>Закрыть</button>
            </Modal>
          )
        ) : editor.kind === 'offer' ? (
          <OfferEditor
            tenant={tenant}
            location={location}
            key={`offer:${editor.item?.id ?? 'new'}:${Boolean(editor.copy)}`}
            initial={editor.item}
            copy={editor.copy}
            current={items.find((i) => i.id === editor.item?.id)}
            specialist={
              specialists.find((s) => s.id === editor.specialist.id) ??
              editor.specialist
            }
            date={date}
            zone={zone}
            presets={templates}
            morePresets={
              presets.hasNextPage
                ? () => {
                  void presets.fetchNextPage();
                }
                : undefined
            }
            busy={busy}
            journal={journal}
            run={run}
            close={() => setEditor(null)}
          />
        ) : (
          <CatalogEditor
            key={`${editor.kind}:${editor.item?.id ?? 'new'}`}
            kind={editor.kind}
            initial={editor.item}
            current={
              editor.kind === 'specialist'
                ? specialists.find((i) => i.id === editor.item?.id)
                : templates.find((i) => i.id === editor.item?.id)
            }
            busy={busy}
            journal={journal}
            run={run}
            close={() =>
              setEditor(
                editor.kind === 'preset' && editor.back
                  ? { kind: 'batch', specialist: editor.back }
                  : null,
              )
            }
          />
        ))}
    </>
  );
}
