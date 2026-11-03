#requires -Version 7.4
param(
    [Parameter(Mandatory)][guid]$User,
    [Parameter(Mandatory)][string]$Email,
    [string]$EnvFile='.env',
    [string[]]$ComposeFile=@('compose.yaml'),
    [string]$Project,
    [Security.SecureString]$Password
)
. "$PSScriptRoot/ops/Common.ps1"
$context=Get-OpsContext -EnvFile $EnvFile -ComposeFile $ComposeFile -Project $Project
if(-not $Password){$Password=Read-Host 'Временный пароль (15–128 символов, ввод скрыт)' -AsSecureString}
$process=[Diagnostics.Process]::new()
$start=[Diagnostics.ProcessStartInfo]::new('docker')
$start.UseShellExecute=$false
$start.RedirectStandardInput=$true
$start.RedirectStandardOutput=$true
$start.RedirectStandardError=$true
$start.StandardInputEncoding=[Text.UTF8Encoding]::new($false)
foreach($argument in ($context.Args+@('run','--rm','--no-deps','--no-TTY','admin','auth','password-set','--user',$User.ToString(),'--email',$Email,'--password-stdin'))){[void]$start.ArgumentList.Add($argument)}
$process.StartInfo=$start
$pointer=[IntPtr]::Zero
$started=$false
try{
    [void]$process.Start()
    $started=$true
    $output=$process.StandardOutput.ReadToEndAsync()
    $errors=$process.StandardError.ReadToEndAsync()
    $pointer=[Runtime.InteropServices.Marshal]::SecureStringToGlobalAllocUnicode($Password)
    $plain=[Runtime.InteropServices.Marshal]::PtrToStringUni($pointer)
    $process.StandardInput.WriteLine($plain)
    $plain=$null
    $process.StandardInput.Close()
    if(-not $process.WaitForExit(30000)){throw 'Password command exceeded its time budget. Check the result before repeating.'}
    [void]$errors.GetAwaiter().GetResult()
    if($process.ExitCode -ne 0){throw 'Password was not confirmed. Check the user, email, database migrations and the 15–128 character policy.'}
    $result=$output.GetAwaiter().GetResult() | ConvertFrom-Json
    if($result.status -ne 'password_set'){throw 'Unexpected password command result.'}
    'Временный пароль установлен. Прежние сессии завершены; при входе потребуется задать свой пароль.'
}finally{
    if($pointer -ne [IntPtr]::Zero){[Runtime.InteropServices.Marshal]::ZeroFreeGlobalAllocUnicode($pointer)}
    $plain=$null
    if($started -and -not $process.HasExited){$process.Kill($true);$process.WaitForExit()}
    $process.Dispose()
}
