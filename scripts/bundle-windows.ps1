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
#   - The .NET SDK (8 or later), for the WiX Toolset that builds the MSI:
#     `dotnet tool restore` installs the version pinned in
#     .config/dotnet-tools.json (5.0.2, under the MS-RL license; WiX 6 and
#     later add the Open Source Maintenance Fee EULA). This script runs it.
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

if (-not (Get-Command dotnet -ErrorAction SilentlyContinue)) {
    throw "The .NET SDK is needed for the WiX Toolset: winget install Microsoft.DotNet.SDK.8"
}
& dotnet tool restore
if ($LASTEXITCODE -ne 0) { throw "dotnet tool restore (WiX) failed" }

# WiX runs from the package `dotnet tool restore` put in NuGet's cache,
# not through `dotnet tool run`, whose own parsing took WiX's -d (as its
# --diagnostics) or passed `--` on to WiX, which refuses it.
$wixVersion = (Get-Content .config\dotnet-tools.json -Raw | ConvertFrom-Json).tools.wix.version
$nuget = if ($env:NUGET_PACKAGES) { $env:NUGET_PACKAGES } else { Join-Path $env:USERPROFILE ".nuget\packages" }
$wix = Get-ChildItem (Join-Path $nuget "wix\$wixVersion\tools") -Recurse -Filter wix.dll |
    Select-Object -First 1
if (-not $wix) { throw "WiX $wixVersion isn't in $nuget after dotnet tool restore" }

New-Item -ItemType Directory -Force dist | Out-Null
$msi = "dist\Picacho-$version-x64.msi"
# MSVC links WebView2's loader statically: no DLL beside the app.
$wixArgs = @(
    $wix.FullName, "build", "-arch", "x64",
    "-define", "Version=$msiVersion",
    "-define", "ExeDir=$exeDir",
    "-define", "WebView2Loader=no",
    "-out", $msi,
    "packaging\windows\picacho.wxs"
)
Write-Host "dotnet $($wixArgs -join ' ')"
# WiX 5 targets .NET 6; roll forward to whichever newer runtime is here.
$env:DOTNET_ROLL_FORWARD = "Major"
& dotnet @wixArgs
if ($LASTEXITCODE -ne 0) { throw "wix build failed (exit $LASTEXITCODE): see WiX's errors above" }
Sign $msi
Write-Host "Built $msi"
