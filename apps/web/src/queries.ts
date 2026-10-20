import { useInfiniteQuery, useQuery } from '@tanstack/react-query';
import { api, ApiError } from './api';

export const keys = {
  organizations: ['organizations'] as const,
  organization: (tenant: string) => ['organization', tenant] as const,
  locations: (tenant: string) => ['organization', tenant, 'locations'] as const,
  tipRecipients: (tenant: string) => ['organization', tenant, 'tips', 'recipients'] as const,
  tipAttempts: (tenant: string) => ['organization', tenant, 'tips', 'attempts'] as const,
};

export function useOrganizations() {
  return useInfiniteQuery({
    queryKey: keys.organizations,
    initialPageParam: null as string | null,
    queryFn: ({ pageParam, signal }) => api.organizations(pageParam, signal),
    getNextPageParam: (page) => page.next_cursor ?? undefined,
  });
}

export function useOrganization(tenant: string) {
  return useQuery({ queryKey: keys.organization(tenant), queryFn: ({ signal }) => api.organization(tenant, signal) });
}

export function useLocations(tenant: string) {
  return useInfiniteQuery({
    queryKey: keys.locations(tenant),
    initialPageParam: null as string | null,
    queryFn: ({ pageParam, signal }) => api.locations(tenant, pageParam, signal),
    getNextPageParam: (page) => page.next_cursor ?? undefined,
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
