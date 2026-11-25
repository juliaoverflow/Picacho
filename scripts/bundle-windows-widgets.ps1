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

Write-Host "Built Windows 11 Widgets provider in $outDir"

if ($Register) {
    $manifest = (Resolve-Path (Join-Path $outDir "AppxManifest.xml")).Path
    $external = (Resolve-Path $outDir).Path
    Add-AppxPackage -Register $manifest -ExternalLocation $external
    Write-Host "Registered Picacho Widgets with the Windows 11 Widgets Board (Win + W)."
}
