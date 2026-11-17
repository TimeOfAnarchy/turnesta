import { errorMessage } from '../api';
import type { BookingCommand, BookingReceipt } from '../bookings';
import { BookingCommands, bookingError } from '../bookings';
export type SaveProps = {
  busy: boolean;
  journal: BookingCommands;
  run: (command?: BookingCommand) => Promise<BookingReceipt>;
  close: () => void;
};
export function FormFailure({
  error,
  pending,
  retry,
  busy,
}: {
  error: unknown;
  pending: boolean;
  retry: () => void;
  busy: boolean;
}) {
  return (
    <>
      {error !== null && (
        <p className="field-error" role="alert">
          {bookingError(error) ?? errorMessage(error)}
        </p>
      )}
      {pending && !busy && (
        <div className="conflict-notice" role="alert">
          <p>
            Сохранение могло выполниться. Повторите прежний запрос, чтобы
            восстановить результат.
          </p>
          <button
            type="button"
            className="button secondary"
            disabled={busy}
            onClick={retry}
          >
            Повторить прежний запрос
          </button>
        </div>
      )}
    </>
  );
}
