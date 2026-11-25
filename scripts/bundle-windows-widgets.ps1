# Builds Picacho's Windows 11 Widgets Board provider (Inbox, Unread Mail,
# Calendar) and optionally registers it locally as a sparse MSIX package.
#
#   powershell -ExecutionPolicy Bypass -File scripts\bundle-windows-widgets.ps1 [-Register]
#
# Needs Windows 11 with the .NET 8 SDK (and Developer Mode enabled for -Register).

param([switch]$Register)
$ErrorActionPreference = "Stop"
Set-Location (Join-Path $PSScriptRoot "..")

$outDir = "dist\PicachoWidgets"
& dotnet publish packaging\windows\widgets\PicachoWidgets.csproj `
    -c Release -r win-x64 --self-contained false -o $outDir
if ($LASTEXITCODE -ne 0) { throw "dotnet publish failed" }

$assetsDir = Join-Path $outDir "Assets"
New-Item -ItemType Directory -Force $assetsDir | Out-Null
Copy-Item assets\brand\icon.png (Join-Path $assetsDir "StoreLogo.png") -Force
Copy-Item assets\brand\icon.png (Join-Path $assetsDir "Square150x150Logo.png") -Force
Copy-Item assets\brand\icon.png (Join-Path $assetsDir "Square44x44Logo.png") -Force
Copy-Item packaging\windows\widgets\AppxManifest.xml (Join-Path $outDir "AppxManifest.xml") -Force

# The widgets' names and descriptions, by language (the manifest's
# ms-resource: names): Strings\<language>\Resources.resw, built into
# resources.pri beside the manifest with the Windows SDK's makepri.
$makepri = Get-ChildItem "${env:ProgramFiles(x86)}\Windows Kits\10\bin\*\x64\makepri.exe" -ErrorAction SilentlyContinue |
    Sort-Object FullName | Select-Object -Last 1
if (-not $makepri) { throw "makepri.exe not found: install the Windows 10/11 SDK" }
$priRoot = Join-Path $env:TEMP "picacho-widgets-pri"
Remove-Item -Recurse -Force $priRoot -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force $priRoot | Out-Null
Copy-Item -Recurse packaging\windows\widgets\Strings (Join-Path $priRoot "Strings")
Copy-Item packaging\windows\widgets\AppxManifest.xml (Join-Path $priRoot "AppxManifest.xml")
$priConfig = Join-Path $priRoot "priconfig.xml"
& $makepri.FullName createconfig /cf $priConfig /dq en-US /o
if ($LASTEXITCODE -ne 0) { throw "makepri createconfig failed" }
& $makepri.FullName new /pr $priRoot /cf $priConfig /mn (Join-Path $priRoot "AppxManifest.xml") `
    /of (Join-Path $outDir "resources.pri") /o
if ($LASTEXITCODE -ne 0) { throw "makepri new failed" }

Write-Host "Built Windows 11 Widgets provider in $outDir"

if ($Register) {
    $manifest = (Resolve-Path (Join-Path $outDir "AppxManifest.xml")).Path
    $external = (Resolve-Path $outDir).Path
    Add-AppxPackage -Register $manifest -ExternalLocation $external
    Write-Host "Registered Picacho Widgets with the Windows 11 Widgets Board (Win + W)."
}
