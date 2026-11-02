import { request } from './api.ts';
export type BatchSource =
  | { kind: 'copy_day'; source_date: string; target_dates: string[] }
  | {
      kind: 'preset_series';
      preset_id: string;
      dates: string[];
      start_time: string;
      end_time: string;
      gap_minutes: number;
    };
export type BatchRequest = {
  id: string;
  specialist_id: string;
  time_zone: string;
  state: 'draft' | 'ready';
  source: BatchSource;
};
export type BatchRow = {
  index: number;
  title: string;
  mode: 'interval' | 'start_only' | 'by_agreement';
  date: string;
  starts_local: string | null;
  ends_local: string | null;
  starts_at: string | null;
  ends_at: string | null;
  time_zone: string;
  places: number;
  state: 'draft' | 'ready';
};
export type BatchConflict = {
  row: number;
  kind: 'duplicate' | 'overlap' | 'internal' | 'booked';
  other_row: number | null;
  title: string;
  starts_local: string | null;
  ends_local: string | null;
};
export type BatchPreview = {
  token: string;
  rows: BatchRow[];
  conflicts: BatchConflict[];
  conflicts_truncated: boolean;
  has_duplicates: boolean;
};
export type BatchReceipt = {
  id: string;
  version: number;
  specialist_id: string;
  dates: string[];
  offer_ids: string[];
};
export type BatchCommand = {
  type: 'save_batch';
  request: BatchRequest;
  preview_token: string;
  allow_alternatives: boolean;
};
const uuid = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
const date = /^\d{4}-\d{2}-\d{2}$/;
export function validBatchReceipt(value: BatchReceipt) {
  return (
    uuid.test(value.id) &&
    value.version === 1 &&
    uuid.test(value.specialist_id) &&
    Array.isArray(value.dates) &&
    value.dates.length > 0 &&
    value.dates.length <= 31 &&
    value.dates.every((d) => typeof d === 'string' && date.test(d)) &&
    Array.isArray(value.offer_ids) &&
    value.offer_ids.length > 0 &&
    value.offer_ids.length <= 200 &&
    value.offer_ids.every((id) => typeof id === 'string' && uuid.test(id)) &&
    new Set(value.offer_ids).size === value.offer_ids.length
  );
}
export function validBatchCommand(value: unknown): value is BatchCommand {
  if (!value || typeof value !== 'object') return false;
  const v = value as BatchCommand,
    r = v.request;
  if (
    Object.keys(v).sort().join(',') !==
      'allow_alternatives,preview_token,request,type' ||
    v.type !== 'save_batch' ||
    typeof v.allow_alternatives !== 'boolean' ||
    typeof v.preview_token !== 'string' ||
    !/^[a-f0-9]{64}$/i.test(v.preview_token) ||
    !r ||
    Object.keys(r).sort().join(',') !==
      'id,source,specialist_id,state,time_zone' ||
    !uuid.test(r.id) ||
    !uuid.test(r.specialist_id) ||
    typeof r.time_zone !== 'string' ||
    r.time_zone.length > 128 ||
    !['draft', 'ready'].includes(r.state) ||
    !r.source
  )
    return false;
  const s = r.source,
    keys = Object.keys(s).sort().join(',');
  const dates =
    s.kind === 'copy_day'
      ? s.target_dates
      : s.kind === 'preset_series'
        ? s.dates
        : null;
  if (
    !Array.isArray(dates) ||
    dates.length < 1 ||
    dates.length > 31 ||
    !dates.every((d) => typeof d === 'string' && date.test(d))
  )
    return false;
  return s.kind === 'copy_day'
    ? keys === 'kind,source_date,target_dates' &&
        typeof s.source_date === 'string' &&
        date.test(s.source_date)
    : s.kind === 'preset_series' &&
        keys === 'dates,end_time,gap_minutes,kind,preset_id,start_time' &&
        uuid.test(s.preset_id) &&
        /^\d{2}:\d{2}$/.test(s.start_time) &&
        /^\d{2}:\d{2}$/.test(s.end_time) &&
        Number.isInteger(s.gap_minutes) &&
        s.gap_minutes >= 0 &&
        s.gap_minutes <= 1440;
}
export async function batchPreview(
  tenant: string,
  point: string,
  body: BatchRequest,
  signal: AbortSignal,
) {
  const value = await request<BatchPreview>(
    `/api/organizations/${encodeURIComponent(tenant)}/locations/${encodeURIComponent(point)}/bookings/batch-preview`,
    {
      method: 'POST',
      body: JSON.stringify(body),
      signal: AbortSignal.any([signal, AbortSignal.timeout(10_000)]),
    },
  );
  if (
    !value ||
    !/^[a-f0-9]{64}$/i.test(value.token) ||
    !Array.isArray(value.rows) ||
    value.rows.length < 1 ||
    value.rows.length > 200 ||
    !value.rows.every(
      (r, i) =>
        r &&
        r.index === i + 1 &&
        typeof r.title === 'string' &&
        date.test(r.date) &&
        r.time_zone === body.time_zone &&
        r.state === body.state &&
        ['interval', 'start_only', 'by_agreement'].includes(r.mode) &&
        Number.isInteger(r.places) &&
        r.places >= 1 &&
        r.places <= 1000 &&
        [r.starts_local, r.ends_local].every(
          (v) =>
            v === null ||
            (typeof v === 'string' &&
              /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}$/.test(v)),
        ),
    ) ||
    !Array.isArray(value.conflicts) ||
    value.conflicts.length > 100 ||
    !value.conflicts.every(
      (c) =>
        c &&
        Number.isInteger(c.row) &&
        c.row >= 1 &&
        c.row <= value.rows.length &&
        ['duplicate', 'overlap', 'internal', 'booked'].includes(c.kind) &&
        (c.other_row === null ||
          (Number.isInteger(c.other_row) &&
            c.other_row >= 1 &&
            c.other_row <= value.rows.length)) &&
        typeof c.title === 'string' &&
        [c.starts_local, c.ends_local].every(
          (v) => v === null || (typeof v === 'string' && v.length === 16),
        ),
    ) ||
    typeof value.has_duplicates !== 'boolean' ||
    typeof value.conflicts_truncated !== 'boolean'
  )
    throw new Error('Invalid batch preview');
  return value;
}
