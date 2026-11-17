import { spawnSync } from 'node:child_process';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('../', import.meta.url));
const allSuites = [
  'core_postgres', 'auth_store_postgres', 'auth_http_postgres', 'delivery_postgres',
  'tips_postgres', 'tips_http_postgres', 'queue_postgres', 'queue_public_postgres',
  'bookings_postgres', 'team_postgres', 'operations_postgres', 'password_postgres',
];

function fail(message) {
  throw new Error(message);
}

function docker(args, { capture = false, input, env = process.env, operation = 'Docker operation' } = {}) {
  const result = spawnSync('docker', args, {
    cwd: root, encoding: 'utf8', env, input,
    stdio: [input === undefined ? 'ignore' : 'pipe', capture ? 'pipe' : 'inherit', 'pipe'],
    maxBuffer: 16 * 1024 * 1024,
  });
  // Docker configuration errors can include credentials; do not echo commands or stderr.
  if (result.error || result.status !== 0) fail(`${operation} failed (exit ${result.status ?? 'unavailable'}). Database checks did not complete; raw Docker stderr is withheld because it can contain credentials.`);
  return result.stdout?.trim();
}

try {
  let envFile = '.env';
  let project;
  const files = [];
  const suites = [];
  const args = process.argv.slice(2);
  while (args.length) {
    const arg = args.shift();
    if (['--env-file', '--file', '--project'].includes(arg)) {
      const value = args.shift();
      if (!value || value.startsWith('--')) fail(`Missing value for ${arg}.`);
      if (arg === '--env-file') envFile = value;
      else if (arg === '--file') files.push(value);
      else project = value;
    } else if (arg === '--help') {
      console.log('Usage: node scripts/test-postgres.mjs [--env-file path] [--file compose.yaml] [--project name] [suite ...]\nRuns migrations and selected suites in turnesta_test on a local development stack. Omit suites to run all.\nSuites: ' + allSuites.join(', '));
      process.exit(0);
    } else if (allSuites.includes(arg)) suites.push(arg);
    else fail(`Unknown argument: ${arg}. Use --help for available suites.`);
  }
  const compose = ['compose', '--env-file', envFile, '--profile', 'tools'];
  for (const file of files.length ? files : ['compose.yaml']) compose.push('--file', file);
  if (project) compose.push('--project-name', project);
  if (root.includes(',')) fail('The repository path cannot contain a comma because Docker --mount uses comma-separated fields.');
  const config = JSON.parse(docker([...compose, 'config', '--format', 'json'], { capture: true, operation: 'Reading Compose configuration' }));
  const bootstrap = config.services?.['db-bootstrap']?.environment;
  const network = config.networks?.default?.name;
  // This tool prepares a sibling test database using development administrator access.
  if (!bootstrap?.PGUSER || !network || !config.services?.api?.ports?.length) {
    fail('Expected a local development Compose stack. Production and restore configurations are not supported.');
  }
  const origin = new URL(config.services.api.environment.TURNESTA_PUBLIC_ORIGIN);
  if (!['localhost', '127.0.0.1', '[::1]'].includes(origin.hostname) || bootstrap.PGHOST !== 'db') {
    fail('Expected a loopback development origin and the Compose db service.');
  }
  const environment = { ...process.env };
  let sourceDatabase;
  for (const [name, service] of [
    ['TEST_MIGRATION_DATABASE_URL', 'migrator'], ['TEST_DATABASE_URL', 'api'],
    ['TEST_ADMIN_DATABASE_URL', 'admin'], ['TEST_WORKER_DATABASE_URL', 'worker'],
    ['TURNESTA_VERIFY_RUNTIME_DATABASE_URL', 'api'],
  ]) {
    const source = config.services[service]?.environment?.DATABASE_URL;
    if (!source) fail('The selected stack is missing a database role configuration.');
    const url = new URL(source);
    if (!['postgres:', 'postgresql:'].includes(url.protocol)) fail('Expected PostgreSQL connection URLs.');
    if (url.hostname !== 'db' || (url.port && url.port !== '5432') || (sourceDatabase && url.pathname !== sourceDatabase)) {
      fail('All database roles must target the same database on the Compose db service.');
    }
    sourceDatabase = url.pathname;
    url.pathname = '/turnesta_test';
    environment[name] = url.toString();
  }
  environment.TURNESTA_DELIVERY_KEY = config.services.worker.environment.TURNESTA_DELIVERY_KEY;
  if (!/^[0-9a-f]{64}$/i.test(environment.TURNESTA_DELIVERY_KEY ?? '')) fail('The local stack requires a valid delivery key.');
  docker([...compose, 'exec', '-T', 'db', 'psql', '--no-psqlrc', '--set=ON_ERROR_STOP=1', '-U', bootstrap.PGUSER, '-d', 'postgres'], {
    input: readFileSync(new URL('./db/create-test-database.sql', import.meta.url), 'utf8'),
    operation: 'Preparing turnesta_test',
  });
  const run = [
    'run', '--rm', '--cpus', '4', '--memory', '4g', '--network', network,
    '--mount', `type=bind,source=${root},target=/workspace`,
    '--mount', 'type=volume,source=turnesta_cargo_registry,target=/usr/local/cargo/registry',
    '--mount', 'type=volume,source=turnesta_cargo_git,target=/usr/local/cargo/git',
    '--mount', 'type=volume,source=turnesta_target,target=/workspace/target',
    '-w', '/workspace',
  ];
  for (const name of ['TEST_MIGRATION_DATABASE_URL', 'TEST_DATABASE_URL', 'TEST_ADMIN_DATABASE_URL', 'TEST_WORKER_DATABASE_URL', 'TURNESTA_VERIFY_RUNTIME_DATABASE_URL', 'TURNESTA_DELIVERY_KEY']) run.push('--env', name);
  run.push(process.env.TURNESTA_DEVTOOL_IMAGE || 'turnesta-devtool:local', 'cargo', 'test', '--package', 'turnesta-api', '--locked', '--test');
  for (const suite of ['postgres', ...(suites.length ? [...new Set(suites)] : allSuites)]) {
    console.log(`Running database suite: ${suite}`);
    docker([...run, suite, '--', '--ignored'], { env: environment, operation: `Database suite ${suite}` });
  }
} catch (error) {
  console.error(error instanceof SyntaxError || error instanceof TypeError ? 'Could not read the local Compose configuration.' : error.message);
  process.exitCode = 1;
}
