import { ErrorNotice, Loading, Modal } from '../components';
import { AppointmentEditor } from './AppointmentEditor';
import type { Controls } from './editorShared';
import { useAppointmentDetails } from './useAppointmentDetails';
export function AppointmentDetails({
  id,
  opening,
  ...props
}: Controls & {
  id: string; opening: string; tenant: string; location: string; role: string;
}) {
  const detail = useAppointmentDetails({ id, opening, ...props });
  if (detail.isSuccess) return <AppointmentEditor {...props} initial={detail.data} />;
  return <Modal title="Запись клиента" description="Подробности записи" close={props.close}>
    {detail.isError ? <ErrorNotice error={detail.error} retry={() => void detail.refetch()} />
      : <Loading label="Загружаем карточку…" />}
  </Modal>;
}
