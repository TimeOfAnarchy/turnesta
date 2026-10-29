export type LiveStatus = 'connecting' | 'live' | 'reconnecting' | 'paused';
export const liveLabels: Record<LiveStatus,string>;
export function watchQueue(options: {
  url: string;
  refresh: () => Promise<boolean>;
  status?: (state: LiveStatus) => void;
  disconnect?: () => void;
  confirm?: () => boolean | void;
}): { refresh(): void; dispose(): void };
