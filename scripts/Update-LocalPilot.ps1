#requires -Version 7.4
param([Parameter(Mandatory)][string]$Stand,[string]$Image='turnesta-api:local',[switch]$Build)
. "$PSScriptRoot/ops/Common.ps1"
$root=Get-OpsRoot
$s=Get-Content -Raw -LiteralPath ([IO.Path]::GetFullPath($Stand,$root)) | ConvertFrom-Json -AsHashtable
if($s.kind -ne 'pilot'){throw 'This script updates pilot stands only.'}
$dc=@('compose','--env-file',$s.env_file,'--project-name',$s.project)
foreach($file in $s.compose_files){$dc+=@('--file',[IO.Path]::GetFullPath($file,$root))}
if($Build){
    # Build output carries no database data, so it is streamed instead of withheld.
    Write-Host 'Building the application image from the working tree...'
    & docker compose --env-file (Join-Path $root '.env') --file (Join-Path $root 'compose.yaml') build api
    if($LASTEXITCODE -ne 0){throw "Image build failed (exit $LASTEXITCODE). The pilot is unchanged."}
}
try{$target=(Invoke-OpsNative -DockerArgs @('image','inspect',$Image)|ConvertFrom-Json -AsHashtable -NoEnumerate)[0].Id}
catch{throw "Image '$Image' is not available locally. Build it with -Build or pass an existing -Image. The pilot is unchanged."}
$envLines=[IO.File]::ReadAllLines($s.env_file)
$pins=@($envLines|Where-Object{$_ -like 'TURNESTA_IMAGE=*'})
if($pins.Count -ne 1){throw 'Expected exactly one TURNESTA_IMAGE line in the stand environment.'}
$previous=$pins[0].Substring('TURNESTA_IMAGE='.Length)
if($previous -eq $target){Write-Host 'The pilot already runs this image; nothing changed.';return}
$origin=@($envLines|Where-Object{$_ -like 'TURNESTA_PUBLIC_ORIGIN=*'})[0].Substring('TURNESTA_PUBLIC_ORIGIN='.Length)

# The default tool image is the pinned one, so the backup matches the current schema.
Write-Host 'Backing up the pilot database before the update...'
$backup=& "$PSScriptRoot/Backup-Database.ps1" -EnvFile $s.env_file -ComposeFile $s.compose_files -Project $s.project
Invoke-OpsNative -DockerArgs @('image','tag',$previous,"$($s.project)-app:previous") | Out-Null

$updated=$envLines|ForEach-Object{if($_ -like 'TURNESTA_IMAGE=*'){"TURNESTA_IMAGE=$target"}else{$_}}
[IO.File]::WriteAllText($s.env_file,($updated -join "`n")+"`n",[Text.UTF8Encoding]::new($false))
try{
    Invoke-OpsNative -DockerArgs @('image','tag',$target,"$($s.project)-app:retained") | Out-Null
    Write-Host 'Applying migrations and restarting api and worker...'
    Invoke-OpsNative -DockerArgs ($dc+@('run','--rm','--no-deps','migrator')) | Out-Null
    Invoke-OpsNative -DockerArgs ($dc+@('up','--detach','--wait','--no-deps','api','worker')) | Out-Null
    $ready=Invoke-RestMethod -Uri "$origin/health/ready" -TimeoutSec 5
    if($ready.status -ne 'ready'){throw 'The pilot API is not ready after the update.'}
    $context=Get-OpsContext -EnvFile $s.env_file -ComposeFile $s.compose_files -Project $s.project
    $inspection=Invoke-OpsInspection -Context $context -VerifyRuntime -CheckDeliveryKey
}catch{
    Write-Warning "Update stopped: $($_.Exception.Message)"
    Write-Warning "The stand now pins $target. The previous image is tagged $($s.project)-app:previous; backup: $backup"
    Write-Warning 'If new migrations were applied, the previous image cannot run on this schema: restore the backup into a separate cluster with Restore-Drill.ps1.'
    throw
}
$report=[ordered]@{format_version=1;updated_at=[DateTime]::UtcNow.ToString('o');project=$s.project;previous_image=$previous;image=$target;
    backup=$backup;migrations=@($inspection.migrations).Count;api_url=$origin;ready=$true}
Write-OpsJsonNew -Path (Join-Path (Split-Path -Parent $s.env_file) ('update-'+[DateTime]::UtcNow.ToString('yyyyMMddTHHmmssZ')+'.json')) -Value $report
$report|ConvertTo-Json
