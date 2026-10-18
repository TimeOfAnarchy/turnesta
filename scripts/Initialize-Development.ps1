$ErrorActionPreference = 'Stop'
$projectRoot = Split-Path -Parent $PSScriptRoot
$destination = Join-Path $projectRoot '.env'
if (Test-Path -LiteralPath $destination) {
    $existing = [IO.File]::ReadAllText($destination)
    if ($existing -notmatch '(?m)^TURNESTA_DELIVERY_KEY=[0-9a-fA-F]{64}\r?$') {
        throw 'Existing .env is preserved. Add TURNESTA_DELIVERY_KEY with 32 random bytes encoded as 64 hex characters.'
    }
    Write-Output 'Existing local .env is valid and was left unchanged.'
    return
}

$bytes = New-Object byte[] 32
$random = [Security.Cryptography.RandomNumberGenerator]::Create()
try { $random.GetBytes($bytes) } finally { $random.Dispose() }
$key = -join ($bytes | ForEach-Object { $_.ToString('x2') })
$template = [IO.File]::ReadAllText((Join-Path $projectRoot '.env.example'))
if ($template -notmatch '(?m)^TURNESTA_DELIVERY_KEY=\r?$') {
    throw 'Expected an empty delivery key placeholder in .env.example.'
}
$content = [regex]::Replace($template, '(?m)^TURNESTA_DELIVERY_KEY=\r?$', 'TURNESTA_DELIVERY_KEY=' + $key)
$stream = [IO.File]::Open($destination, [IO.FileMode]::CreateNew, [IO.FileAccess]::Write, [IO.FileShare]::None)
try {
    $writer = [IO.StreamWriter]::new($stream, [Text.UTF8Encoding]::new($false))
    try { $writer.Write($content) } finally { $writer.Dispose() }
} finally { $stream.Dispose() }
[Array]::Clear($bytes, 0, $bytes.Length)
Write-Output 'Created ignored local .env with a random delivery encryption key. Keep it with database backups.'
