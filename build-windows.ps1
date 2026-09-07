$ErrorActionPreference = 'Stop'
Push-Location $PSScriptRoot
try {
    cargo build --release --locked
    if ($LASTEXITCODE -ne 0) { throw 'La compilation a échoué.' }
    $runtimeDestination = Join-Path $PSScriptRoot 'target\release\drivers'
    New-Item -ItemType Directory -Force -Path $runtimeDestination | Out-Null
    foreach ($dllName in @('rtlsdr.dll', 'msvcr100.dll', 'pthreadVC2.dll')) {
        $dllSource = Join-Path $PSScriptRoot "drivers\$dllName"
        if (-not (Test-Path -LiteralPath $dllSource)) { throw "Bibliothèque manquante : $dllSource" }
        Copy-Item -LiteralPath $dllSource -Destination $runtimeDestination
    }
    Write-Host 'QuasarSDR RX prêt dans target\release (conserver le sous-dossier drivers).'
} finally {
    Pop-Location
}
