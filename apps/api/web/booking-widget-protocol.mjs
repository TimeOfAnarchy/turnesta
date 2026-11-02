export function validWidgetMessage(data, channel, type) {
  const fields =
    type === 'turnesta:resize' ? 'channel,height,type,v' : 'channel,type,v';
  return (
    data !== null &&
    typeof data === 'object' &&
    !Array.isArray(data) &&
    Object.keys(data).sort().join(',') === fields &&
    data.v === 1 &&
    data.type === type &&
    data.channel === channel &&
    (type !== 'turnesta:resize' ||
      (Number.isInteger(data.height) &&
        data.height > 0 &&
        data.height <= 100_000))
  );
}

export async function probeBookingStorage(storage, locks) {
  if (!storage || !locks?.request) throw new Error('storage_unavailable');
  const key = `turnesta:probe:${crypto.randomUUID()}`;
  await locks.request(
    key,
    { mode: 'exclusive', signal: AbortSignal.timeout(3000) },
    () => {
      try {
        storage.setItem(key, key);
        if (storage.getItem(key) !== key)
          throw new Error('storage_unavailable');
      } finally {
        storage.removeItem(key);
      }
    },
  );
}
