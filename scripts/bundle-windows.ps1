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

# The installer's dialogs (WixUI_Minimal) come from WiX's UI extension, a
# NuGet package of the same version (MS-RL), unpacked under target\.
$uiDir = "target\wix\wixtoolset.ui.wixext.$wixVersion"
$uiExt = Join-Path $uiDir "wixext5\WixToolset.UI.wixext.dll"
if (-not (Test-Path $uiExt)) {
    New-Item -ItemType Directory -Force $uiDir | Out-Null
    $nupkg = Join-Path $uiDir "package.zip"
    Invoke-WebRequest -UseBasicParsing -OutFile $nupkg `
        "https://api.nuget.org/v3-flatcontainer/wixtoolset.ui.wixext/$wixVersion/wixtoolset.ui.wixext.$wixVersion.nupkg"
    # Its code runs in the build: checked against the package as published
    # (5.0.2) before it is unpacked. Another WiX version needs its hash here.
    $known = @{ "5.0.2" = "5EF2C707614B9F70B6BBADD2D4ABCB4124EFEE215E9B16BFBC80113079A604C7" }
    $hash = (Get-FileHash -Algorithm SHA256 $nupkg).Hash
    if (-not $known.ContainsKey($wixVersion) -or $known[$wixVersion] -ne $hash) {
        Remove-Item -Force $nupkg
        throw "WiX's UI extension $wixVersion doesn't match its known SHA-256 ($hash)"
    }
    Expand-Archive -Force $nupkg $uiDir
    if (-not (Test-Path $uiExt)) { throw "WiX's UI extension wasn't found in its package" }
}

New-Item -ItemType Directory -Force dist | Out-Null
# WiX 5 targets .NET 6; roll forward to whichever newer runtime is here.
$env:DOTNET_ROLL_FORWARD = "Major"
# An installer in each language of packaging\windows\picacho.<culture>.wxl:
# English's is Picacho-<version>-x64.msi, the others' end in their culture.
foreach ($loc in Get-ChildItem packaging\windows\picacho.*.wxl) {
    $culture = $loc.Name -replace '^picacho\.(.+)\.wxl$', '$1'
    $suffix = if ($culture -eq "en-US") { "" } else { "-$culture" }
    $msi = "dist\Picacho-$version-x64$suffix.msi"
    # MSVC links WebView2's loader statically: no DLL beside the app.
    $wixArgs = @(
        $wix.FullName, "build", "-arch", "x64",
        "-ext", $uiExt,
        "-culture", $culture,
        "-loc", $loc.FullName,
        "-define", "Version=$msiVersion",
        "-define", "ExeDir=$exeDir",
        "-define", "WebView2Loader=no",
        "-out", $msi,
        "packaging\windows\picacho.wxs"
    )
    Write-Host "dotnet $($wixArgs -join ' ')"
    & dotnet @wixArgs
    if ($LASTEXITCODE -ne 0) { throw "wix build failed (exit $LASTEXITCODE): see WiX's errors above" }
    Sign $msi
    Write-Host "Built $msi"
}
