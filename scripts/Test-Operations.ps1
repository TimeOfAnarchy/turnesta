#requires -Version 7.4
param([Parameter(Mandatory)][string]$Bundle,[Parameter(Mandatory)][string]$EnvFile,[Parameter(Mandatory)][string[]]$ComposeFile,[Parameter(Mandatory)][string]$Project,[string]$ToolImage)
. "$PSScriptRoot/ops/Common.ps1"
# Run against a stopped synthetic pilot with at least one pending invitation.
$context=Get-OpsContext -EnvFile $EnvFile -ComposeFile $ComposeFile -Project $Project -ToolImage $ToolImage
$before=Invoke-OpsInspection -Context $context -Fingerprints -CheckDeliveryKey
if($before.encrypted_invitations_verified -lt 1){throw 'This check requires a pending encrypted invitation in a quiescent synthetic pilot.'}
$directory=New-OpsPrivateDirectory -Parent (Join-Path (Get-OpsRoot) '.local/operations') -Prefix 'checks'
$sentinel=Join-Path $directory 'sentinel.txt'
Write-OpsFileNew -Path $sentinel -Text 'retain this content'
$refused=$false
try{Write-OpsFileNew -Path $sentinel -Text 'unexpected replacement'}catch [IO.IOException]{$refused=$true}
if(-not $refused -or [IO.File]::ReadAllText($sentinel) -ne 'retain this content'){throw 'CreateNew protection failed.'}
$badKey=New-OpsSecret
$actualKey=$context.Config.services.api.environment.TURNESTA_DELIVERY_KEY
try{
    $context.Config.services.api.environment.TURNESTA_DELIVERY_KEY=$badKey
    $refused=$false
    try{Invoke-OpsInspection -Context $context -CheckDeliveryKey | Out-Null}catch{$refused=$true}
    if(-not $refused){throw 'An incorrect delivery key was accepted.'}
}finally{$context.Config.services.api.environment.TURNESTA_DELIVERY_KEY=$actualKey}
$after=Invoke-OpsInspection -Context $context -Fingerprints -CheckDeliveryKey
if(($before.tables|ConvertTo-Json -Depth 5 -Compress) -ne ($after.tables|ConvertTo-Json -Depth 5 -Compress)){throw 'Inspection changed synthetic data or another writer is still active.'}
$corrupt=Join-Path $directory 'corrupt-bundle'
[void](New-Item -ItemType Directory -Path $corrupt)
foreach($name in @('database.dump','manifest.json')){
    $source=[IO.File]::OpenRead((Join-Path $Bundle $name))
    $target=[IO.File]::Open((Join-Path $corrupt $name),[IO.FileMode]::CreateNew)
    try{$source.CopyTo($target)}finally{$source.Dispose();$target.Dispose()}
}
$stream=[IO.File]::OpenWrite((Join-Path $corrupt 'database.dump'))
try{$stream.WriteByte(0)}finally{$stream.Dispose()}
$refused=$false
# A nonexistent source file proves checksum rejection precedes Docker setup.
try{& "$PSScriptRoot/Restore-Drill.ps1" -Bundle $corrupt -SourceEnvFile (Join-Path $directory 'does-not-exist.env') | Out-Null}catch{
    if($_.Exception.Message -notlike 'Backup checksum or size differs*'){throw}
    $refused=$true
}
if(-not $refused){throw 'A damaged archive was accepted.'}
Write-OpsJsonNew -Path (Join-Path $directory 'checks.json') -Value @{success=$true;create_new_refused_overwrite=$true;wrong_key_refused=$true;inspection_preserved_data=$true;corrupt_archive_rejected_before_docker=$true;verified_pending_invitations=$after.encrypted_invitations_verified}
Write-Output (Join-Path $directory 'checks.json')
