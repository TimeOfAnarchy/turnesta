#requires -Version 7.4
param([string]$EnvFile='.env',[string[]]$ComposeFile=@('compose.yaml'),[string]$Project,[ValidateRange(1,86400)][int]$MaxDueAgeSeconds=300,[switch]$FailOnBacklog,[string]$ToolImage)
. "$PSScriptRoot/ops/Common.ps1"
$context=Get-OpsContext -EnvFile $EnvFile -ComposeFile $ComposeFile -Project $Project -ToolImage $ToolImage
$inspection=Invoke-OpsInspection -Context $context -VerifyRuntime -CheckDeliveryKey -MaxDueAgeSeconds $MaxDueAgeSeconds
$containers=Invoke-OpsNative -DockerArgs ($context.Args+@('--profile','pilot','--profile','tools','ps','--all','--format','json'))
$states=@()
foreach($line in ($containers -split "`n")){
    if($line.Trim()){
        $item=$line|ConvertFrom-Json -AsHashtable
        $states+=@{service=$item.Service;state=$item.State;health=$item.Health}
    }
}
$failedJobs=0
foreach($state in $inspection.delivery.states){if($state.state -eq 'failed'){$failedJobs+=$state.count}}
$report=@{format_version=1;inspected_at=[DateTime]::UtcNow.ToString('o');project=$context.Config.name;containers=$states;database=$inspection;failed_jobs=$failedJobs;
    note='A running worker container does not prove delivery. Review due backlog and failed states; SMTP receipt is separate.'}
$report|ConvertTo-Json -Depth 40
if($FailOnBacklog -and ($inspection.delivery.overdue -gt 0 -or $inspection.delivery.expired_pending -gt 0 -or $failedJobs -gt 0)){throw 'Delivery backlog or failed jobs require attention.'}
