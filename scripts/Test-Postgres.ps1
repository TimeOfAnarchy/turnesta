param([string]$EnvFile = '.env')

$ErrorActionPreference = 'Stop'
$projectPath = Split-Path -Parent $PSScriptRoot
Push-Location -LiteralPath $projectPath

function Invoke-Docker {
    param([string[]]$DockerArgs)
    & docker @DockerArgs
    if ($LASTEXITCODE -ne 0) {
        throw 'Docker command failed; PostgreSQL checks did not complete.'
    }
}

function Test-DatabaseUrl {
    param([string]$Url)
    $connection = [Uri]$Url
    return $connection.GetLeftPart([UriPartial]::Authority) + '/turnesta_test'
}

try {
    $composeArgs = @('compose', '--env-file', $EnvFile, '--profile', 'tools')
    $configJson = & docker @composeArgs config --format json
    if ($LASTEXITCODE -ne 0) { throw 'Could not read local Compose configuration.' }
    $config = ($configJson -join "`n") | ConvertFrom-Json

    $bootstrap = $config.services.'db-bootstrap'.environment
    Get-Content -Raw -LiteralPath 'scripts/db/create-test-database.sql' |
        & docker @composeArgs exec -T db psql --no-psqlrc --set=ON_ERROR_STOP=1 -U $bootstrap.PGUSER -d postgres
    if ($LASTEXITCODE -ne 0) { throw 'Could not prepare the dedicated test database.' }

    $runArgs = @(
        'run', '--rm', '--cpus', '4', '--memory', '4g',
        '--network', $config.networks.default.name,
        '--mount', "type=bind,source=$projectPath,target=/workspace",
        '--mount', 'type=volume,source=turnesta_cargo_registry,target=/usr/local/cargo/registry',
        '--mount', 'type=volume,source=turnesta_target,target=/workspace/target',
        '-w', '/workspace',
        '-e', ('TEST_MIGRATION_DATABASE_URL=' + (Test-DatabaseUrl $config.services.migrator.environment.DATABASE_URL)),
        '-e', ('TEST_DATABASE_URL=' + (Test-DatabaseUrl $config.services.api.environment.DATABASE_URL)),
        '-e', ('TEST_ADMIN_DATABASE_URL=' + (Test-DatabaseUrl $config.services.admin.environment.DATABASE_URL)),
        '-e', ('TEST_WORKER_DATABASE_URL=' + (Test-DatabaseUrl $config.services.worker.environment.DATABASE_URL)),
        'turnesta-devtool:local', 'cargo', 'test', '--package', 'turnesta-api', '--locked', '--test'
    )
    Invoke-Docker -DockerArgs ($runArgs + @('postgres', '--', '--ignored'))
    Invoke-Docker -DockerArgs ($runArgs + @('core_postgres', '--', '--ignored'))
    Invoke-Docker -DockerArgs ($runArgs + @('auth_store_postgres', '--', '--ignored'))
    Invoke-Docker -DockerArgs ($runArgs + @('auth_http_postgres', '--', '--ignored'))
    Invoke-Docker -DockerArgs ($runArgs + @('delivery_postgres', '--', '--ignored'))
}
finally {
    Pop-Location
}
