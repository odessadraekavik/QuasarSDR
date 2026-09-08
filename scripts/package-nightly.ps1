$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest
Push-Location (Split-Path $PSScriptRoot -Parent)
try {
    $metadata = cargo metadata --no-deps --format-version 1 --locked | ConvertFrom-Json
    if ($LASTEXITCODE -ne 0) { throw 'Cargo metadata failed.' }
    $package = $metadata.packages | Where-Object name -EQ 'quasar-sdr'
    $paris = [TimeZoneInfo]::ConvertTimeBySystemTimeZoneId([DateTimeOffset]::UtcNow, 'Romance Standard Time')
    $name = 'v{0}-dev.{1}' -f $package.version, $paris.ToString('yyyyMMdd')
    $output = Join-Path $metadata.target_directory 'nightly'
    $stage = Join-Path $output ('stage-' + [Guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Force -Path "$stage/drivers" | Out-Null
    Copy-Item -LiteralPath "$($metadata.target_directory)/release/quasar-sdr.exe" -Destination $stage

    # Distribution officielle figée et vérifiée, indépendante des DLL locales.
    $runtime = Join-Path $output 'rtl-sdr-blog-V1.4.0.zip'
    Invoke-WebRequest 'https://github.com/rtlsdrblog/rtl-sdr-blog/releases/download/V1.4.0/Release.zip' -OutFile $runtime
    $expected = '7ef33f1304647f65e5e0fde43637a73d54f076e91e651a3cecc4f55a17fd9815'
    if ((Get-FileHash $runtime -Algorithm SHA256).Hash.ToLowerInvariant() -ne $expected) {
        throw 'RTL-SDR archive checksum mismatch.'
    }
    $unpack = Join-Path $output ('runtime-' + [Guid]::NewGuid().ToString('N'))
    Expand-Archive -LiteralPath $runtime -DestinationPath $unpack
    foreach ($dll in @('rtlsdr.dll', 'msvcr100.dll', 'pthreadVC2.dll')) {
        $matches = @(Get-ChildItem $unpack -Recurse -File -Filter $dll | Where-Object { $_.Directory.Name -eq 'x64' })
        if ($matches.Count -ne 1) { throw "Missing or ambiguous x64 runtime: $dll" }
        Copy-Item -LiteralPath $matches[0].FullName -Destination "$stage/drivers"
    }
    $zip = Join-Path $output 'QuasarSDR-Windows-x64.zip'
    Compress-Archive -Path "$stage/*" -DestinationPath $zip -Force
    $hash = (Get-FileHash $zip -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($env:GITHUB_OUTPUT) {
        "release_name=$name" | Out-File $env:GITHUB_OUTPUT -Append -Encoding utf8
        "sha256=$hash" | Out-File $env:GITHUB_OUTPUT -Append -Encoding utf8
    }
    Write-Host "$name — $zip — SHA256 $hash"
} finally {
    Pop-Location
}
