<#
.SYNOPSIS
  Build the native Windows app: WinUI 3 interface + Rust engine (odm_ffi.dll).

.EXAMPLE
  apps\windows\build.ps1                 # this PC's architecture, app folder only
  apps\windows\build.ps1 -Installer      # also the setup .exe (needs Inno Setup 6)
  apps\windows\build.ps1 -Arch arm64

.NOTES
  Needs the .NET 10 SDK and Rust (rustup). Run from anywhere in the repository.
#>
param(
    [ValidateSet('x64', 'arm64')][string]$Arch = $(if ($env:PROCESSOR_ARCHITECTURE -eq 'ARM64') { 'arm64' } else { 'x64' }),
    [switch]$Installer
)
$ErrorActionPreference = 'Stop'
$root = (Resolve-Path "$PSScriptRoot\..\..").Path
Set-Location $root

function Step($text) { Write-Host "`n==> $text" -ForegroundColor Blue }
function Check($what) { if ($LASTEXITCODE -ne 0) { throw "$what failed ($LASTEXITCODE)" } }

$version = (Select-String -Path Cargo.toml -Pattern '^version = "([^"]+)"' | Select-Object -First 1).Matches.Groups[1].Value
$target = if ($Arch -eq 'arm64') { 'aarch64-pc-windows-msvc' } else { 'x86_64-pc-windows-msvc' }
$platform = if ($Arch -eq 'arm64') { 'ARM64' } else { 'x64' }
$out = "$root\dist\windows-native\$Arch"
$app = "$out\app"

Step "Rust engine ($target)"
rustup target add $target | Out-Null
cargo build --release -p odm-ffi --target $target
Check 'cargo build'
$dll = "$root\target\$target\release\odm_ffi.dll"
if (-not (Test-Path $dll)) { throw "missing $dll" }

if (-not (Test-Path "$root\extension\dist\chromium")) {
    $bash = Get-Command bash -ErrorAction SilentlyContinue
    if ($bash) {
        Step 'Browser extension'
        & $bash.Source extension/build.sh
        Check 'extension/build.sh'
    } else {
        Write-Warning 'bash not found: the browser extension is not bundled (run extension/build.sh)'
    }
}

Step "Windows app ($Arch)"
if (Test-Path $app) { Remove-Item -Recurse -Force $app }
dotnet publish apps\windows\OpenDownloadManager.csproj -c Release -r "win-$Arch" -p:Platform=$platform "-p:OdmFfiDll=$dll" -o $app
Check 'dotnet publish'
if (-not (Test-Path "$app\OpenDownloadManager.exe")) { throw "missing $app\OpenDownloadManager.exe" }
if (-not (Test-Path "$app\odm_ffi.dll")) { throw "missing $app\odm_ffi.dll" }

if ($Installer) {
    Step 'Installer'
    $iscc = (Get-Command iscc -ErrorAction SilentlyContinue).Source
    if (-not $iscc) {
        $iscc = @("${env:ProgramFiles(x86)}\Inno Setup 6\ISCC.exe", "$env:ProgramFiles\Inno Setup 6\ISCC.exe") | Where-Object { Test-Path $_ } | Select-Object -First 1
    }
    if (-not $iscc) { throw 'Inno Setup 6 not found (winget install JRSoftware.InnoSetup)' }
    & $iscc "/DAppVersion=$version" "/DArch=$Arch" "/DSourceDir=$app" "/DOutputDir=$root\dist\windows-native" apps\windows\installer\odm.iss
    Check 'iscc'
}

Step "Done: $app"
