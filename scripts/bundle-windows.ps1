# Builds Picacho for Windows and its installer, dist\Picacho-<version>-x64.msi.
#
#   powershell -ExecutionPolicy Bypass -File scripts\bundle-windows.ps1 [release|debug]
#
# Needs, on Windows:
#   - Rust's MSVC toolchain (rustup's default on Windows) and Visual Studio's
#     C++ build tools with a Windows SDK: GPUI compiles its shaders with the
#     SDK's fxc.exe in release builds, and RC.EXE builds the icon resource.
#     (Release builds can't be made on Linux for that reason.)
#   - Perl for OpenSSL's vendored build: Strawberry Perl, or Git for
#     Windows' perl.exe on PATH.
#   - WiX Toolset 3 (candle.exe and light.exe on PATH, or WIX set by its
#     installer), for the MSI.
#
# Optional built-in sign-in registrations are read from .env, as by
# scripts/bundle.sh (see README, "Connect email"). The MSI isn't signed:
# set PICACHO_SIGNTOOL_CERT (a .pfx) and PICACHO_SIGNTOOL_PASSWORD to sign
# picacho.exe and the MSI with signtool.

param([ValidateSet("release", "debug")][string]$Profile = "release")
$ErrorActionPreference = "Stop"
Set-Location (Join-Path $PSScriptRoot "..")

if (Test-Path .env) {
    foreach ($line in Get-Content .env) {
        if ($line -match '^\s*([A-Za-z_][A-Za-z0-9_]*)\s*=\s*"?(.*?)"?\s*$' -and $line -notmatch '^\s*#') {
            [Environment]::SetEnvironmentVariable($Matches[1], $Matches[2], "Process")
        }
    }
}

$target = "x86_64-pc-windows-msvc"
$cargoArgs = @("build", "--locked", "--target", $target)
if ($Profile -eq "release") { $cargoArgs += "--release" }
& cargo @cargoArgs
if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }
$exeDir = "target\$target\$Profile"

$version = (Select-String -Path Cargo.toml -Pattern '^version\s*=\s*"([^"]+)"' |
    Select-Object -First 1).Matches[0].Groups[1].Value
# MSI versions are major.minor.build, numbers only.
$msiVersion = ($version -split '[-+]')[0]

function Sign($path) {
    if (-not $env:PICACHO_SIGNTOOL_CERT) { return }
    & signtool sign /fd SHA256 /td SHA256 /tr http://timestamp.digicert.com `
        /f $env:PICACHO_SIGNTOOL_CERT /p $env:PICACHO_SIGNTOOL_PASSWORD $path
    if ($LASTEXITCODE -ne 0) { throw "signtool failed for $path" }
}
Sign "$exeDir\picacho.exe"

$wixBin = if ($env:WIX) { Join-Path $env:WIX "bin" } else { "" }
$candle = if ($wixBin) { Join-Path $wixBin "candle.exe" } else { "candle.exe" }
$light = if ($wixBin) { Join-Path $wixBin "light.exe" } else { "light.exe" }

New-Item -ItemType Directory -Force dist | Out-Null
$obj = "target\$target\$Profile\picacho.wixobj"
$msi = "dist\Picacho-$version-x64.msi"
# MSVC links WebView2's loader statically: no DLL beside the app.
& $candle -nologo -arch x64 "-dVersion=$msiVersion" "-dExeDir=$exeDir" `
    "-dWebView2Loader=no" -out $obj packaging\windows\picacho.wxs
if ($LASTEXITCODE -ne 0) { throw "candle failed" }
# ICE validation needs Windows Installer's service; per-user installs trip
# ICE38/ICE64/ICE91 by design.
& $light -nologo -sice:ICE38 -sice:ICE64 -sice:ICE91 -out $msi $obj
if ($LASTEXITCODE -ne 0) { throw "light failed" }
Sign $msi
Write-Host "Built $msi"
