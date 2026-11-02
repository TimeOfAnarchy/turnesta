#requires -Version 7.4
param([string]$SourceEnvFile='.env',[ValidateRange(1024,65535)][int]$ApiPort=8086,[ValidateRange(1024,65535)][int]$MailPort=8026,[string]$MailhogImage='mailhog/mailhog')
. "$PSScriptRoot/ops/Common.ps1"
if($ApiPort -eq $MailPort){throw 'Pilot API and MailHog ports must differ.'}
foreach($port in @($ApiPort,$MailPort)){
    $listener=[Net.Sockets.TcpListener]::new([Net.IPAddress]::Loopback,$port)
    try{$listener.Start()}catch{throw "Local port $port is unavailable."}finally{$listener.Stop()}
}
$source=Get-OpsContext -EnvFile $SourceEnvFile
$mailImage=(Invoke-OpsNative -DockerArgs @('image','inspect',$MailhogImage)|ConvertFrom-Json -AsHashtable -NoEnumerate)[0].Id
$stand=New-OpsStand -Kind pilot -Image $source.Image -DatabaseImage $source.DatabaseImage -DeliveryKey (New-OpsSecret) -MailhogImage $mailImage -ApiPort $ApiPort -MailPort $MailPort
Write-Host 'Starting a new synthetic pilot; the existing project is unchanged...'
Write-Host "Stand metadata: $($stand.Directory)"
Invoke-OpsNative -DockerArgs ($stand.Args+@('up','--detach','--wait','db')) | Out-Null
Invoke-OpsNative -DockerArgs ($stand.Args+@('run','--rm','--no-deps','db-bootstrap')) | Out-Null
Invoke-OpsNative -DockerArgs ($stand.Args+@('run','--rm','--no-deps','migrator')) | Out-Null
$owner=Invoke-OpsNative -DockerArgs ($stand.Args+@('run','--rm','--no-deps','admin','core','organization-create','--name','Локальная пилотная клиника','--owner-name','Владелец пилота')) | ConvertFrom-Json -AsHashtable
Invoke-OpsNative -DockerArgs ($stand.Args+@('up','--detach','--wait','api','mailhog')) | Out-Null
$ready=Invoke-RestMethod -Uri "$($stand.Origin)/health/ready" -TimeoutSec 5
if($ready.status -ne 'ready'){throw 'The pilot API is not reachable from the host.'}
Invoke-RestMethod -Uri "http://127.0.0.1:$MailPort/api/v2/messages" -TimeoutSec 5 | Out-Null
Invoke-OpsNative -DockerArgs ($stand.Args+@('run','--rm','--no-deps','admin','auth','invite','--user',$owner.owner.id,'--email','pilot-owner@example.test')) | Out-Null
Invoke-OpsNative -DockerArgs ($stand.Args+@('up','--detach','--wait','worker')) | Out-Null
$delivered=$false
for($attempt=0;$attempt -lt 20;$attempt++){
    $mail=Invoke-RestMethod -Uri "http://127.0.0.1:$MailPort/api/v2/messages" -TimeoutSec 5
    if($mail.total -gt 0){$delivered=$true;break}
    Start-Sleep -Milliseconds 500
}
if(-not $delivered){throw 'Owner invitation has not reached the private MailHog. Stand retained for inspection.'}
$report=@{format_version=1;project=$stand.Project;directory=$stand.Directory;api_url=$stand.Origin;mailhog_url="http://127.0.0.1:$MailPort";owner_email='pilot-owner@example.test';organization_id=$owner.organization.id;synthetic_only=$true;tips='disabled';ready_at=[DateTime]::UtcNow.ToString('o')}
Write-OpsJsonNew -Path (Join-Path $stand.Directory 'pilot.json') -Value $report
$report | ConvertTo-Json
