import { useQuery } from '@tanstack/react-query';
import { appointmentApi } from '../appointments';
export function useAppointmentDetails({
  id,
  opening,
  tenant,
  location,
}: { id: string; opening: string; tenant: string; location: string }) {
  const detail = useQuery({
    queryKey: ['organization', tenant, 'appointment-detail', location, id, opening],
    // A StrictMode remount shares this opening's request. A new click always has a new key.
    queryFn: () => appointmentApi.get(tenant, location, id, AbortSignal.timeout(10_000)),
    staleTime: Infinity,
    gcTime: 0,
    retry: false,
    refetchOnReconnect: false,
    refetchOnWindowFocus: false,
  });
  return detail;
}
