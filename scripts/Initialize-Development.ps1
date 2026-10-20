$ErrorActionPreference = 'Stop'
$projectRoot = Split-Path -Parent $PSScriptRoot
$destination = Join-Path $projectRoot '.env'
function New-LocalKey {
    $randomBytes = New-Object byte[] 32
    $generator = [Security.Cryptography.RandomNumberGenerator]::Create()
    try { $generator.GetBytes($randomBytes) } finally { $generator.Dispose() }
    $encoded = -join ($randomBytes | ForEach-Object { $_.ToString('x2') })
    [Array]::Clear($randomBytes, 0, $randomBytes.Length)
    return $encoded
}
if (Test-Path -LiteralPath $destination) {
    $existing = [IO.File]::ReadAllText($destination)
    if ($existing -notmatch '(?m)^TURNESTA_DELIVERY_KEY=[0-9a-fA-F]{64}\r?$') {
        throw 'Existing .env is preserved. Add TURNESTA_DELIVERY_KEY with 32 random bytes encoded as 64 hex characters.'
    }
    $additions = @()
    if ($existing -match '(?m)^TURNESTA_MOCK_PAYMENT_KEY=') {
        if ($existing -notmatch '(?m)^TURNESTA_MOCK_PAYMENT_KEY=[0-9a-fA-F]{64}\r?$') {
            throw 'Existing .env is preserved. TURNESTA_MOCK_PAYMENT_KEY must contain 32 random bytes encoded as 64 hex characters.'
        }
    } else { $additions += 'TURNESTA_MOCK_PAYMENT_KEY=' + (New-LocalKey) }
    if ($existing -notmatch '(?m)^TURNESTA_TIPS_MODE=') { $additions += 'TURNESTA_TIPS_MODE=mock' }
    if ($additions.Count) {
        [IO.File]::AppendAllText($destination, "`n" + ($additions -join "`n") + "`n", [Text.UTF8Encoding]::new($false))
        Write-Output 'Preserved existing environment values and appended missing local mock configuration.'
    } else { Write-Output 'Existing local .env is valid and was left unchanged.' }
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
if ($content -notmatch '(?m)^TURNESTA_MOCK_PAYMENT_KEY=\r?$') { throw 'Expected an empty mock payment key placeholder.' }
$content = [regex]::Replace($content, '(?m)^TURNESTA_MOCK_PAYMENT_KEY=\r?$', 'TURNESTA_MOCK_PAYMENT_KEY=' + (New-LocalKey))
$stream = [IO.File]::Open($destination, [IO.FileMode]::CreateNew, [IO.FileAccess]::Write, [IO.FileShare]::None)
try {
    $writer = [IO.StreamWriter]::new($stream, [Text.UTF8Encoding]::new($false))
    try { $writer.Write($content) } finally { $writer.Dispose() }
} finally { $stream.Dispose() }
[Array]::Clear($bytes, 0, $bytes.Length)
Write-Output 'Created ignored local .env with independent delivery and mock payment keys. Keep secrets with database backups.'
