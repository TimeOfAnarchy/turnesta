param(
    [string]$EnvFile = '.env',
    [ValidateSet('core_postgres', 'auth_store_postgres', 'auth_http_postgres', 'delivery_postgres', 'tips_postgres', 'tips_http_postgres', 'queue_postgres', 'queue_public_postgres')]
    [string[]]$Suites = @('core_postgres', 'auth_store_postgres', 'auth_http_postgres', 'delivery_postgres', 'tips_postgres', 'tips_http_postgres', 'queue_postgres', 'queue_public_postgres')
)

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
    foreach ($suite in $Suites) {
        Invoke-Docker -DockerArgs ($runArgs + @($suite, '--', '--ignored'))
    }
}
finally {
    Pop-Location
}
