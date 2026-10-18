import { useInfiniteQuery, useQuery } from '@tanstack/react-query';
import { api } from './api';

export const keys = {
  organizations: ['organizations'] as const,
  organization: (tenant: string) => ['organization', tenant] as const,
  locations: (tenant: string) => ['organization', tenant, 'locations'] as const,
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
