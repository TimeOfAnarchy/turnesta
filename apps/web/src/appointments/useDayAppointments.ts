import { useInfiniteQuery } from '@tanstack/react-query';
import { appointmentApi } from '../appointments';
export function useDayAppointments(
  tenant: string,
  location: string,
  specialist: string,
  date: string,
  cancelled: boolean,
  enabled = true,
) {
  return useInfiniteQuery({
    queryKey: [
      'organization',
      tenant,
      'bookings',
      location,
      'appointments',
      specialist,
      date,
      cancelled,
    ],
    initialPageParam: null as string | null,
    enabled,
    queryFn: ({ pageParam, signal }) =>
      appointmentApi.list(
        tenant,
        location,
        specialist,
        date,
        cancelled,
        pageParam,
        signal,
      ),
    getNextPageParam: (p) => p.next_cursor ?? undefined,
    retry: false,
    refetchInterval: 10_000,
  });
}
