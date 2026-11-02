import { useInfiniteQuery, useQuery } from '@tanstack/react-query';
import { api, ApiError, readRecoveryInterval } from './api';
import type { TicketStatus } from './api';

export const keys = {
  organizations: ['organizations'] as const,
  organization: (tenant: string) => ['organization', tenant] as const,
  locations: (tenant: string) => ['organization', tenant, 'locations'] as const,
  tipRecipients: (tenant: string) => ['organization', tenant, 'tips', 'recipients'] as const,
  tipAttempts: (tenant: string) => ['organization', tenant, 'tips', 'attempts'] as const,
  queues: (tenant: string, location: string) => ['organization', tenant, 'queues', location] as const,
  queue: (tenant: string, queue: string) => ['organization', tenant, 'queue', queue] as const,
  tickets: (tenant: string, queue: string) => [...keys.queue(tenant, queue), 'tickets'] as const,
  stations: (tenant:string, queue:string) => [...keys.queue(tenant,queue),'stations'] as const,
  services: (tenant: string, queue: string) => [...keys.queue(tenant, queue), 'services'] as const,
};

export function useQueueStations(tenant:string, queue:string) {
  return useInfiniteQuery({queryKey:keys.stations(tenant,queue),initialPageParam:null as string|null,
    queryFn:({pageParam,signal})=>api.queueStations(tenant,queue,pageParam,signal),
    getNextPageParam:page=>page.next_cursor ?? undefined,retry:false});
}

export function useQueueServices(tenant: string, queue: string) {
  return useInfiniteQuery({ queryKey: keys.services(tenant, queue), initialPageParam: null as string | null,
    queryFn: ({ pageParam, signal }) => api.queueServices(tenant, queue, pageParam, signal),
    getNextPageParam: page => page.next_cursor ?? undefined, retry: false });
}

export function useOrganizations() {
  return useInfiniteQuery({
    queryKey: keys.organizations,
    initialPageParam: null as string | null,
    queryFn: ({ pageParam, signal }) => api.organizations(pageParam, signal),
    refetchInterval: query => query.state.error ? readRecoveryInterval(query.state.error) : 15_000,
    getNextPageParam: (page) => page.next_cursor ?? undefined,
  });
}

export function useOrganization(tenant: string) {
  return useQuery({ queryKey: keys.organization(tenant), queryFn: ({ signal }) => api.organization(tenant, signal), refetchInterval: query => query.state.error ? readRecoveryInterval(query.state.error) : 15_000 });
}

export function useLocations(tenant: string) {
  return useInfiniteQuery({
    queryKey: keys.locations(tenant),
    initialPageParam: null as string | null,
    queryFn: ({ pageParam, signal }) => api.locations(tenant, pageParam, signal),
    getNextPageParam: (page) => page.next_cursor ?? undefined,
  });
}

export function useQueues(tenant: string, location: string) {
  return useInfiniteQuery({
    queryKey: keys.queues(tenant, location), enabled: Boolean(location),
    initialPageParam: null as string | null,
    queryFn: ({ pageParam, signal }) => api.queues(tenant, location, pageParam, signal),
    getNextPageParam: (page) => page.next_cursor ?? undefined,
  });
}

export function useQueueTickets(tenant: string, queue: string, status: TicketStatus) {
  return useInfiniteQuery({
    queryKey: [...keys.tickets(tenant, queue), status],
    initialPageParam: null as number | null,
    queryFn: ({ pageParam, signal }) => api.tickets(tenant, queue, status, pageParam, signal),
    getNextPageParam: (page) => page.next_cursor ?? undefined,
    retry: false,
  });
}

function retryTips(failures: number, error: Error): boolean {
  return !(error instanceof ApiError && (error.code === 'tips_disabled' || error.status < 500)) && failures < 1;
}

export function useTipRecipients(tenant: string) {
  return useInfiniteQuery({
    queryKey: keys.tipRecipients(tenant),
    initialPageParam: null as string | null,
    queryFn: ({ pageParam, signal }) => api.tipRecipients(tenant, pageParam, signal),
    getNextPageParam: (page) => page.next_cursor ?? undefined,
    retry: retryTips,
  });
}

export function useTipAttempts(tenant: string) {
  return useInfiniteQuery({
    queryKey: keys.tipAttempts(tenant),
    initialPageParam: null as string | null,
    queryFn: ({ pageParam, signal }) => api.tipAttempts(tenant, pageParam, signal),
    getNextPageParam: (page) => page.next_cursor ?? undefined,
    retry: retryTips,
  });
}
