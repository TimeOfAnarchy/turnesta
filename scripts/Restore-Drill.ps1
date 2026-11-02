#requires -Version 7.4
param([Parameter(Mandatory)][string]$Bundle,[string]$SourceEnvFile='.env',[string[]]$SourceComposeFile=@('compose.yaml'),[string]$SourceProject,[string]$ToolImage)
. "$PSScriptRoot/ops/Common.ps1"
$bundlePath=[IO.Path]::GetFullPath($Bundle)
$manifest=Get-Content -Raw -LiteralPath (Join-Path $bundlePath 'manifest.json') | ConvertFrom-Json -AsHashtable
$archive=Join-Path $bundlePath 'database.dump'
if($manifest.format_version -ne 1 -or $manifest.archive.name -ne 'database.dump' -or $manifest.archive.sha256 -notmatch '^[a-f0-9]{64}$'){throw 'Unsupported backup manifest.'}
if((Get-Item -LiteralPath $archive).Length -ne $manifest.archive.bytes -or (Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash.ToLowerInvariant() -ne $manifest.archive.sha256){throw 'Backup checksum or size differs; restore was not started.'}
$source=Get-OpsContext -EnvFile $SourceEnvFile -ComposeFile $SourceComposeFile -Project $SourceProject -ToolImage $ToolImage
[void](Invoke-OpsNative -DockerArgs @('image','inspect',$manifest.database_image))
$stand=New-OpsStand -Kind restore -Image $source.Image -DatabaseImage $manifest.database_image -DeliveryKey $source.Config.services.api.environment.TURNESTA_DELIVERY_KEY
$clock=[Diagnostics.Stopwatch]::StartNew()
$succeeded=$false
try {
    Write-Host 'Restoring into a new isolated PostgreSQL cluster (no published ports, no worker)...'
    Invoke-OpsNative -DockerArgs ($stand.Args+@('up','--detach','--wait','db')) | Out-Null
    Invoke-OpsNative -DockerArgs ($stand.Args+@('run','--rm','--no-deps','db-bootstrap')) | Out-Null
    $context=Get-OpsContext -EnvFile $stand.EnvFile -ComposeFile $stand.ComposeFile -Project $stand.Project
    Assert-OpsRestoreIsolation -Context $context
    $exists=Invoke-OpsSql -Context $context -Database postgres -Sql "SELECT EXISTS(SELECT 1 FROM pg_database WHERE datname='turnesta_restore')"
    if($exists -ne 'f'){throw 'Restore target already exists; nothing was overwritten.'}
    Invoke-OpsSql -Context $context -Database postgres -Sql 'CREATE DATABASE turnesta_restore OWNER turnesta_migrator TEMPLATE template0' | Out-Null
    Invoke-OpsSql -Context $context -Sql 'REVOKE ALL ON DATABASE turnesta_restore FROM PUBLIC; GRANT CONNECT ON DATABASE turnesta_restore TO turnesta_runtime,turnesta_provisioner,turnesta_delivery_worker; GRANT CONNECT,CREATE ON DATABASE turnesta_restore TO turnesta_migrator' | Out-Null
    Invoke-OpsNative -DockerArgs @('exec','-i',$context.Container,'sh','-c','export PGPASSWORD="$POSTGRES_PASSWORD"; exec "$@"','turnesta-restore',
        'pg_restore','--no-password','--exit-on-error','--single-transaction','--username',$context.User,'--dbname','turnesta_restore') -InputFile $archive | Out-Null
    $inspection=Invoke-OpsInspection -Context $context -Fingerprints:([bool]$manifest.quiescent_declared) -VerifyRuntime -CheckDeliveryKey
    if(($manifest.migrations|ConvertTo-Json -Depth 5 -Compress) -ne ($inspection.migrations|ConvertTo-Json -Depth 5 -Compress)){throw 'Restored migrations differ from the archive manifest.'}
    if($manifest.quiescent_declared -and ($manifest.tables|ConvertTo-Json -Depth 5 -Compress) -ne ($inspection.tables|ConvertTo-Json -Depth 5 -Compress)){throw 'Restored table fingerprints differ from the declared quiescent source.'}
    Invoke-OpsNative -DockerArgs ($stand.Args+@('up','--detach','--wait','api')) | Out-Null
    Assert-OpsRestoreIsolation -Context $context
    $health=Invoke-OpsNative -DockerArgs ($stand.Args+@('exec','-T','api','curl','--fail','--silent','http://127.0.0.1:8080/health/ready')) | ConvertFrom-Json
    if($health.status -ne 'ready'){throw 'Restored API is not ready.'}
    $clock.Stop()
    Write-OpsJsonNew -Path (Join-Path $stand.Directory 'restore-report.json') -Value @{format_version=1;success=$true;bundle_sha256=$manifest.archive.sha256;restored_at=[DateTime]::UtcNow.ToString('o');elapsed_seconds=$clock.Elapsed.TotalSeconds;data_fingerprints_compared=[bool]$manifest.quiescent_declared;inspection=$inspection;api_ready=$true;worker_started=$false;ports_published=$false}
    $succeeded=$true
    Write-Output (Join-Path $stand.Directory 'restore-report.json')
}finally{
    Invoke-OpsNative -DockerArgs ($stand.Args+@('stop','api','db')) | Out-Null
    if(-not $succeeded){Write-Host "Restore did not complete. Isolated data is retained in $($stand.Directory); no working database was changed."}
}
