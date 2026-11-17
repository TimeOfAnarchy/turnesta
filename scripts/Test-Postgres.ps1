param(
    [string]$EnvFile = '.env',
    [string[]]$ComposeFile = @('compose.yaml'),
    [string]$Project,
    [ValidateSet('core_postgres', 'auth_store_postgres', 'auth_http_postgres', 'delivery_postgres', 'tips_postgres', 'tips_http_postgres', 'queue_postgres', 'queue_public_postgres', 'bookings_postgres', 'team_postgres', 'operations_postgres', 'password_postgres')]
    [string[]]$Suites = @('core_postgres', 'auth_store_postgres', 'auth_http_postgres', 'delivery_postgres', 'tips_postgres', 'tips_http_postgres', 'queue_postgres', 'queue_public_postgres', 'bookings_postgres', 'team_postgres', 'operations_postgres', 'password_postgres')
)

$ErrorActionPreference = 'Stop'
# Compatibility entry point; Node owns database targeting and test orchestration.
$runner = Join-Path $PSScriptRoot 'test-postgres.mjs'
$arguments = @($runner, '--env-file', $EnvFile)
foreach ($file in $ComposeFile) { $arguments += @('--file', $file) }
if ($Project) { $arguments += @('--project', $Project) }
$arguments += $Suites
& node @arguments
if ($LASTEXITCODE -ne 0) { throw 'PostgreSQL checks did not complete.' }
