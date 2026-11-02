#requires -Version 7.4
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

function Get-OpsRoot { return [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '../..')) }

function Invoke-OpsNative {
    param([string[]]$DockerArgs,[string]$InputFile,[string]$OutputFile,[hashtable]$Environment=@{},[int]$TimeoutSeconds=600)
    $start=[Diagnostics.ProcessStartInfo]::new('docker')
    $start.UseShellExecute=$false
    $start.RedirectStandardOutput=$true
    $start.RedirectStandardError=$true
    $start.RedirectStandardInput=$true
    foreach($argument in $DockerArgs){[void]$start.ArgumentList.Add($argument)}
    foreach($key in $Environment.Keys){$start.Environment[$key]=[string]$Environment[$key]}
    $process=[Diagnostics.Process]::new();$process.StartInfo=$start
    $output=$null;$inputStream=$null;$started=$false
    $clock=[Diagnostics.Stopwatch]::StartNew()
    try {
        [void]$process.Start()
        $started=$true
        $errorTask=$process.StandardError.ReadToEndAsync()
        if($OutputFile){
            $output=[IO.File]::Open($OutputFile,[IO.FileMode]::CreateNew,[IO.FileAccess]::Write,[IO.FileShare]::None)
            $outputTask=$process.StandardOutput.BaseStream.CopyToAsync($output)
        }else{$outputTask=$process.StandardOutput.ReadToEndAsync()}
        if($InputFile){
            $inputStream=[IO.File]::OpenRead($InputFile)
            $inputTask=$inputStream.CopyToAsync($process.StandardInput.BaseStream)
            if(-not $inputTask.Wait($TimeoutSeconds*1000)){throw 'Docker input exceeded its time budget.'}
        }
        $process.StandardInput.Close()
        $remaining=[Math]::Max(0,[int]($TimeoutSeconds*1000-$clock.Elapsed.TotalMilliseconds))
        if(-not $process.WaitForExit($remaining)){
            $process.Kill($true)
            throw 'Docker operation exceeded its time budget. Incomplete output is retained; do not treat it as a valid backup.'
        }
        $text=$outputTask.GetAwaiter().GetResult()
        [void]$errorTask.GetAwaiter().GetResult()
        if($process.ExitCode -ne 0){throw "Docker operation failed (exit $($process.ExitCode)). Raw stderr is withheld because database errors can contain private data."}
        if(-not $OutputFile){return $text.Trim()}
    }finally{
        if($inputStream){$inputStream.Dispose()};if($output){$output.Dispose()}
        if($started -and -not $process.HasExited){$process.Kill($true);$process.WaitForExit()}
        $process.Dispose()
    }
}

function New-OpsPrivateDirectory {
    param([string]$Parent,[string]$Prefix)
    if(-not $IsWindows){throw 'These local operations scripts currently require Windows ACL support.'}
    $parentPath=[IO.Path]::GetFullPath($Parent)
    if(-not (Test-Path -LiteralPath $parentPath)){[void](New-Item -ItemType Directory -Path $parentPath)}
    $name=$Prefix+'-'+[DateTime]::UtcNow.ToString('yyyyMMddTHHmmssZ')+'-'+[Guid]::NewGuid().ToString('N').Substring(0,10)
    $path=Join-Path $parentPath $name
    [void](New-Item -ItemType Directory -Path $path)
    $acl=[Security.AccessControl.DirectorySecurity]::new()
    $acl.SetAccessRuleProtection($true,$false)
    $sid=[Security.Principal.WindowsIdentity]::GetCurrent().User
    foreach($identity in @($sid,[Security.Principal.SecurityIdentifier]::new('S-1-5-18'),[Security.Principal.SecurityIdentifier]::new('S-1-5-32-544'))){
        $rule=[Security.AccessControl.FileSystemAccessRule]::new($identity,'FullControl','ContainerInherit,ObjectInherit','None','Allow')
        $acl.AddAccessRule($rule)
    }
    Set-Acl -LiteralPath $path -AclObject $acl
    return $path
}

function Write-OpsFileNew {
    param([string]$Path,[string]$Text)
    $stream=[IO.File]::Open($Path,[IO.FileMode]::CreateNew,[IO.FileAccess]::Write,[IO.FileShare]::None)
    try{$bytes=[Text.UTF8Encoding]::new($false).GetBytes($Text);$stream.Write($bytes,0,$bytes.Length)}finally{$stream.Dispose()}
}
function Write-OpsJsonNew {param([string]$Path,$Value) Write-OpsFileNew -Path $Path -Text ($Value|ConvertTo-Json -Depth 40)}
function New-OpsSecret {return [Convert]::ToHexString([Security.Cryptography.RandomNumberGenerator]::GetBytes(32)).ToLowerInvariant()}

function Get-OpsContext {
    param([string]$EnvFile='.env',[string[]]$ComposeFile=@('compose.yaml'),[string]$Project,[string]$ToolImage)
    $root=Get-OpsRoot
    $envPath=[IO.Path]::GetFullPath($EnvFile,$root)
    if(-not (Test-Path -LiteralPath $envPath -PathType Leaf)){throw 'Environment file not found.'}
    $arguments=@('compose','--env-file',$envPath)
    foreach($file in $ComposeFile){$arguments+=@('--file',[IO.Path]::GetFullPath($file,$root))}
    if($Project){$arguments+=@('--project-name',$Project)}
    $config=Invoke-OpsNative -DockerArgs ($arguments+@('--profile','tools','--profile','pilot','config','--format','json')) | ConvertFrom-Json -AsHashtable
    $container=Invoke-OpsNative -DockerArgs ($arguments+@('ps','--quiet','db'))
    if(-not $container -or $container.Contains("`n")){throw 'Expected exactly one running database container.'}
    $actual=(Invoke-OpsNative -DockerArgs @('inspect',$container)|ConvertFrom-Json -AsHashtable -NoEnumerate)[0]
    if($actual.Config.Labels['com.docker.compose.project'] -ne $config.name -or $actual.Config.Labels['com.docker.compose.service'] -ne 'db'){throw 'Database container does not match the selected Compose project.'}
    $url=[Uri]$config.services.migrator.environment.DATABASE_URL
    $database=[Uri]::UnescapeDataString($url.AbsolutePath.TrimStart('/'))
    if($database -notmatch '^[a-zA-Z_][a-zA-Z0-9_]{0,62}$'){throw 'This local tool requires a simple database identifier.'}
    if(-not $ToolImage){$ToolImage=$config.services.api.image}
    $image=(Invoke-OpsNative -DockerArgs @('image','inspect',$ToolImage)|ConvertFrom-Json -AsHashtable -NoEnumerate)[0].Id
    $apiContainer=Invoke-OpsNative -DockerArgs ($arguments+@('ps','--all','--quiet','api'))
    $sourceImage=if($apiContainer){(Invoke-OpsNative -DockerArgs @('inspect',$apiContainer)|ConvertFrom-Json -AsHashtable -NoEnumerate)[0].Image}else{$image}
    return @{Root=$root;Args=$arguments;Config=$config;Container=$container;Database=$database;User=$config.services.'db-bootstrap'.environment.PGUSER;Image=$image;SourceImage=$sourceImage;DatabaseImage=$actual.Image;EnvFile=$envPath;Network=$config.networks.default.name}
}

function Invoke-OpsSql {
    param($Context,[string]$Sql,[string]$Database)
    if(-not $Database){$Database=$Context.Database}
    return Invoke-OpsNative -DockerArgs @('exec','--env','PGOPTIONS=-c statement_timeout=30000 -c lock_timeout=2000',$Context.Container,
        'sh','-c','export PGPASSWORD="$POSTGRES_PASSWORD"; exec "$@"','turnesta-ops',
        'psql','--no-psqlrc','--no-password','--set','ON_ERROR_STOP=1','-At','--username',$Context.User,'--dbname',$Database,'--command',$Sql)
}

function Invoke-OpsInspection {
    param($Context,[switch]$Fingerprints,[switch]$VerifyRuntime,[switch]$CheckDeliveryKey,[int]$MaxDueAgeSeconds=300)
    $environment=@{DATABASE_URL=$Context.Config.services.migrator.environment.DATABASE_URL;RUST_LOG='error';TURNESTA_VERIFY_RUNTIME_DATABASE_URL=$Context.Config.services.api.environment.DATABASE_URL}
    $arguments=@('run','--rm','--network',$Context.Network,'--env','DATABASE_URL','--env','RUST_LOG','--env','TURNESTA_VERIFY_RUNTIME_DATABASE_URL')
    if($CheckDeliveryKey){$environment.TURNESTA_DELIVERY_KEY=$Context.Config.services.api.environment.TURNESTA_DELIVERY_KEY;$arguments+=@('--env','TURNESTA_DELIVERY_KEY')}
    $arguments+=@($Context.Image,'ops','inspect','--max-due-age-seconds',[string]$MaxDueAgeSeconds)
    if($Fingerprints){$arguments+='--fingerprints'}
    if($VerifyRuntime){$arguments+='--verify-runtime'}
    if($CheckDeliveryKey){$arguments+='--check-delivery-key'}
    return Invoke-OpsNative -DockerArgs $arguments -Environment $environment | ConvertFrom-Json -AsHashtable
}

function New-OpsStand {
    param([ValidateSet('pilot','restore')][string]$Kind,[string]$Image,[string]$DatabaseImage,[string]$DeliveryKey,[string]$MailhogImage,[int]$ApiPort=8086,[int]$MailPort=8026)
    $root=Get-OpsRoot
    $directory=New-OpsPrivateDirectory -Parent (Join-Path $root '.local/operations') -Prefix $Kind
    $project='turnesta-'+$Kind+'-'+[Guid]::NewGuid().ToString('N').Substring(0,12)
    $existing=Invoke-OpsNative -DockerArgs @('volume','ls','--quiet','--filter',"label=com.docker.compose.project=$project")
    if($existing){throw 'Unexpected existing rehearsal volumes; no changes made.'}
    # Retain the selected images even when development rebuilds a mutable tag.
    Invoke-OpsNative -DockerArgs @('image','tag',$Image,"${project}-app:retained") | Out-Null
    Invoke-OpsNative -DockerArgs @('image','tag',$DatabaseImage,"${project}-db:retained") | Out-Null
    $database=if($Kind -eq 'pilot'){'turnesta_pilot'}else{'turnesta_restore'}
    $initial=if($Kind -eq 'pilot'){$database}else{'postgres'}
    # Cookies ignore ports: use another host than the main 127.0.0.1 development app.
    $origin=if($Kind -eq 'pilot'){"http://localhost:$ApiPort"}else{'http://127.0.0.1:8080'}
    $settings=[ordered]@{
        TURNESTA_IMAGE=$Image;TURNESTA_DB_IMAGE=$DatabaseImage;TURNESTA_DATABASE=$database;TURNESTA_INITIAL_DATABASE=$initial;TURNESTA_BOOTSTRAP_DATABASE=$initial;
        POSTGRES_USER='turnesta';POSTGRES_PASSWORD=(New-OpsSecret);TURNESTA_MIGRATOR_DB_PASSWORD=(New-OpsSecret);TURNESTA_API_DB_PASSWORD=(New-OpsSecret);TURNESTA_ADMIN_DB_PASSWORD=(New-OpsSecret);TURNESTA_WORKER_DB_PASSWORD=(New-OpsSecret);
        TURNESTA_DELIVERY_KEY=$DeliveryKey;TURNESTA_PUBLIC_ORIGIN=$origin;TURNESTA_BIND_ADDR=$(if($Kind -eq 'pilot'){'0.0.0.0:8080'}else{'127.0.0.1:8080'});
        TURNESTA_PILOT_API_PORT=$ApiPort;TURNESTA_PILOT_MAIL_PORT=$MailPort;TURNESTA_MAILHOG_IMAGE=$MailhogImage
    }
    foreach($value in $settings.Values){if([string]$value -match '[\r\n]'){throw 'Invalid environment value.'}}
    $envPath=Join-Path $directory '.env'
    Write-OpsFileNew -Path $envPath -Text (($settings.GetEnumerator()|ForEach-Object{"$($_.Key)=$($_.Value)"}) -join "`n")
    $files=@('deploy/compose.local.yaml')
    if($Kind -eq 'pilot'){$files+='deploy/compose.local.ports.yaml'}
    $arguments=@('compose','--env-file',$envPath,'--project-name',$project)
    foreach($file in $files){$arguments+=@('--file',(Join-Path $root $file))}
    Write-OpsJsonNew -Path (Join-Path $directory 'stand.json') -Value @{format_version=1;project=$project;env_file=$envPath;compose_files=$files;kind=$Kind;created_at=[DateTime]::UtcNow.ToString('o')}
    return @{Directory=$directory;Project=$project;EnvFile=$envPath;ComposeFile=$files;Args=$arguments;Database=$database;Origin=$origin}
}

function Assert-OpsRestoreIsolation {
    param($Context)
    $network=(Invoke-OpsNative -DockerArgs @('network','inspect',$Context.Network)|ConvertFrom-Json -AsHashtable -NoEnumerate)[0]
    if(-not $network.Internal){throw 'Restore network is not internal.'}
    $ids=Invoke-OpsNative -DockerArgs ($Context.Args+@('--profile','pilot','--profile','tools','ps','--all','--quiet'))
    foreach($id in ($ids -split "`n")){
        if(-not $id.Trim()){continue}
        $container=(Invoke-OpsNative -DockerArgs @('inspect',$id)|ConvertFrom-Json -AsHashtable -NoEnumerate)[0]
        if($container.Config.Labels['com.docker.compose.service'] -notin @('db','api')){throw 'Unexpected service in the restore project.'}
        if($container.HostConfig.PortBindings -and $container.HostConfig.PortBindings.Count -gt 0){throw 'Restore container publishes host ports.'}
    }
}
