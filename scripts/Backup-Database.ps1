#requires -Version 7.4
param([string]$EnvFile='.env',[string[]]$ComposeFile=@('compose.yaml'),[string]$Project,[string]$OutputDirectory=(Join-Path $PSScriptRoot '../.local/backups'),[switch]$Quiescent,[string]$ToolImage)
. "$PSScriptRoot/ops/Common.ps1"
$context=Get-OpsContext -EnvFile $EnvFile -ComposeFile $ComposeFile -Project $Project -ToolImage $ToolImage
$directory=New-OpsPrivateDirectory -Parent $OutputDirectory -Prefix 'backup'
$archive=Join-Path $directory 'database.dump'
$started=[DateTime]::UtcNow
Write-Host 'Inspecting the source and creating a private logical backup...'
$before=Invoke-OpsInspection -Context $context -Fingerprints:$Quiescent
$version=Invoke-OpsNative -DockerArgs @('exec',$context.Container,'pg_dump','--version')
Invoke-OpsNative -DockerArgs @('exec',$context.Container,'sh','-c','export PGPASSWORD="$POSTGRES_PASSWORD"; exec "$@"','turnesta-backup',
    'pg_dump','--format=custom','--no-password','--lock-wait-timeout=5000','--username',$context.User,'--dbname',$context.Database) -OutputFile $archive
$after=Invoke-OpsInspection -Context $context -Fingerprints:$Quiescent
if(($before.migrations|ConvertTo-Json -Depth 5 -Compress) -ne ($after.migrations|ConvertTo-Json -Depth 5 -Compress)){throw 'Migrations changed during backup. Incomplete bundle retained without a manifest; repeat after migration work finishes.'}
if($Quiescent -and ($before.tables|ConvertTo-Json -Depth 5 -Compress) -ne ($after.tables|ConvertTo-Json -Depth 5 -Compress)){throw 'Declared quiescent data changed during backup. Bundle retained without a manifest.'}
$commit=& git -C $context.Root rev-parse HEAD 2>$null
if($LASTEXITCODE -ne 0){$commit=$null}
$dirty=@(& git -C $context.Root status --porcelain 2>$null).Count -gt 0
$manifest=@{format_version=1;started_at=$started.ToString('o');completed_at=[DateTime]::UtcNow.ToString('o');database=$context.Database;
    pg_dump_version=$version;server_version_num=$after.server_version_num;database_image=$context.DatabaseImage;source_image=$context.SourceImage;tool_image=$context.Image;git_commit=$commit;git_dirty=$dirty;
    archive=@{name='database.dump';bytes=(Get-Item -LiteralPath $archive).Length;sha256=(Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash.ToLowerInvariant()};
    migrations=$after.migrations;quiescent_declared=[bool]$Quiescent;tables=$(if($Quiescent){$after.tables}else{$null});
    metadata_snapshot='Inspections are separate from the pg_dump snapshot. Quiescent fingerprints require the operator to stop all writers.'}
Write-OpsJsonNew -Path (Join-Path $directory 'manifest.json') -Value $manifest
Write-Output $directory
