import { readFile, writeFile } from 'node:fs/promises';
import { isDeepStrictEqual, parseArgs } from 'node:util';

const { values, positionals } = parseArgs({
  options: { check: { type: 'boolean' } },
  allowPositionals: true,
});
const url = positionals[0] ?? 'http://127.0.0.1:8080/openapi.json';
const response = await fetch(url, { signal: AbortSignal.timeout(10_000) });
if (!response.ok) throw new Error(`GET ${url} returned HTTP ${response.status}`);
const document = await response.json();
if (!document.openapi || !document.paths) throw new Error('Expected an OpenAPI document');
const snapshot = new URL('../../openapi.json', import.meta.url);
if (values.check) {
  const saved = JSON.parse(await readFile(snapshot, 'utf8'));
  if (!isDeepStrictEqual(document, saved)) {
    throw new Error('openapi.json differs from the API. Refresh it with npm run api:snapshot.');
  }
} else {
  await writeFile(snapshot, JSON.stringify(document, null, 2) + '\n');
}
